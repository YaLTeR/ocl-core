//! Thin and safe OpenCL API function wrappers.
//!
//!
//!
//!

use std::ptr;
use std::mem;
use std::ffi::CString;
use std::iter;
// #[cfg(any(feature="kernel_debug_sleep", target_os="windows"))] use std::thread;
// #[cfg(any(feature="kernel_debug_sleep", target_os="windows"))] use std::time::Duration;
use std::thread;
use std::time::Duration;
use std::env;
use std::fmt::Debug;
use libc::{size_t, c_void};
use num::FromPrimitive;

use ffi::{cl_GLuint, cl_GLint, cl_GLenum};
use ffi::{clCreateFromGLBuffer, clCreateFromGLRenderbuffer, clCreateFromGLTexture,
    clCreateFromGLTexture2D, clCreateFromGLTexture3D, clEnqueueAcquireGLObjects,
    clEnqueueReleaseGLObjects};

use ffi::{self, cl_bool, cl_int, cl_uint, cl_platform_id, cl_device_id, cl_device_type,
    cl_device_info, cl_platform_info, cl_context, cl_context_info, cl_context_properties,
    cl_image_format, cl_image_desc, cl_kernel, cl_program_build_info, cl_mem, cl_mem_info,
    cl_mem_flags, cl_mem_object_type, cl_buffer_create_type, cl_event, cl_program,
    cl_addressing_mode, cl_filter_mode, cl_command_queue_info, cl_command_queue, cl_image_info,
    cl_sampler, cl_sampler_info, cl_program_info, cl_kernel_info, cl_kernel_arg_info,
    cl_kernel_work_group_info, cl_event_info, cl_profiling_info};
use error::{Error as OclError, Result as OclResult};
use ::{OclPrm, PlatformId, DeviceId, Context, ContextProperties, ContextInfo, ContextInfoResult,
    MemFlags, CommandQueue, Mem, MemObjectType, Program, Kernel, ClNullEventPtr, Sampler,
    KernelArg, DeviceType, ImageFormat, ImageDescriptor, CommandExecutionStatus, AddressingMode,
    FilterMode, PlatformInfo, PlatformInfoResult, DeviceInfo, DeviceInfoResult, CommandQueueInfo,
    CommandQueueInfoResult, MemInfo, MemInfoResult, ImageInfo, ImageInfoResult, SamplerInfo,
    SamplerInfoResult, ProgramInfo, ProgramInfoResult, ProgramBuildInfo, ProgramBuildInfoResult,
    KernelInfo, KernelInfoResult, KernelArgInfo, KernelArgInfoResult, KernelWorkGroupInfo,
    KernelWorkGroupInfoResult, ClEventPtrRef, ClWaitListPtr, EventInfo, EventInfoResult,
    ProfilingInfo, ProfilingInfoResult, CreateContextCallbackFn, UserDataPtr, ClPlatformIdPtr,
    ClDeviceIdPtr, ClContextPtr, EventCallbackFn, BuildProgramCallbackFn, MemMigrationFlags,
    MapFlags, BufferRegion, BufferCreateType, OpenclVersion, ClVersions, Status,
    CommandQueueProperties, MemMap, AsMem, MemCmdRw, MemCmdAll, Event, ImageFormatParseResult};


// [TODO]: Do proper auto-detection of available OpenGL context type.
#[cfg(target_os="macos")]
const CL_GL_SHARING_EXT: &'static str = "cl_APPLE_gl_sharing";
#[cfg(not(target_os="macos"))]
const CL_GL_SHARING_EXT: &'static str = "cl_khr_gl_sharing";

const KERNEL_DEBUG_SLEEP_DURATION_MS: u64 = 150;


/// Don't be a dummy. Buckle your `_dummy_callback`.
pub extern "C" fn _dummy_event_callback(_: ffi::cl_event, _: i32, _: *mut c_void) {}


/// If `event_status` is `CommandExecutionStatus::Complete`, the `cl_event`
/// pointed to by `user_data` will be set to the same.
///
/// `user_data` must be a `cl_event` which has not yet had its destructor run
/// (`::release_event`).
///
/// `src_event_ptr` is not used and does not need anything special done with
/// its destructor (it will already have been managed by the call to `::set_event_callback`.
///
pub extern "C" fn _complete_user_event(src_event_ptr: cl_event, event_status: i32,
        user_data: *mut c_void)
{
    #[cfg(not(feature = "event_debug_print"))]
    let _ = src_event_ptr;

    if event_status == CommandExecutionStatus::Complete as i32 && !user_data.is_null() {
        let tar_event_ptr = user_data as *mut _ as cl_event;

        unsafe {
            let user_event = Event::from_raw(tar_event_ptr);

            #[cfg(feature = "event_debug_print")]
            println!("::_complete_user_event: Setting event complete for: \
                source: {:?}, target: {:?}...", src_event_ptr, &user_event);

            ::set_user_event_status(&user_event, CommandExecutionStatus::Complete).unwrap();
        }

        #[cfg(feature = "event_debug_print")]
        println!("  - Event status has been set to 'CommandExecutionStatus::Complete' \
            for event: {:?}", tar_event_ptr);
    } else {
        match CommandExecutionStatus::from_i32(event_status) {
            Some(status_enum) => panic!("ocl_core::_complete_event: User data is null or event \
                is not complete. Status: '{:?}'", status_enum),
            None => eval_errcode(event_status, (), "clSetEventCallback",
                &format!("src_event_ptr: {:?}", src_event_ptr)).unwrap(),
        }
    }
}

//============================================================================
//============================================================================
//=========================== SUPPORT FUNCTIONS ==============================
//============================================================================
//============================================================================

/// Evaluates `errcode` and returns an `Err` with a failure message if it is
/// not 0 (Status::CL_SUCCESS).
///
#[inline(always)]
fn eval_errcode<T>(errcode: cl_int, result: T, cl_fn_name: &'static str, fn_info: &str)
        -> OclResult<T>
{
    OclError::eval_errcode(errcode, result, cl_fn_name, fn_info)
}

/// Maps options of slices to pointers and a length.
fn resolve_event_ptrs<En: ClNullEventPtr, Ewl: ClWaitListPtr>(wait_list: Option<Ewl>,
            new_event: Option<En>) -> (cl_uint, *const cl_event, *mut cl_event)
{
    // If the wait list is empty or if its containing option is none, map to (0, null),
    // otherwise map to the length and pointer:
    let (wait_list_len, wait_list_ptr) = match wait_list {
        Some(wl) => {
            if wl.count() > 0 {
                (wl.count(), unsafe { wl.as_ptr_ptr() } as *const cl_event)
            } else {
                (0, ptr::null() as *const cl_event)
            }
        },
        None => (0, ptr::null() as *const cl_event),
    };

    let new_event_ptr = match new_event {
        Some(mut ne) => ne.alloc_new(),
        None => ptr::null_mut() as *mut cl_event,
    };

    (wait_list_len, wait_list_ptr, new_event_ptr)
}

/// Converts an array option reference into a pointer to the contained array.
fn resolve_work_dims(work_dims: Option<&[usize; 3]>) -> *const size_t {
    match work_dims {
        Some(w) => w as *const [usize; 3] as *const size_t,
        None => 0 as *const size_t,
    }
}

/// If the program pointed to by `cl_program` for any of the devices listed in
/// `device_ids` has a build log of any length, it will be returned as an
/// errcode result.
///
pub fn program_build_err<D: ClDeviceIdPtr>(program: &Program, device_ids: &[D]) -> OclResult<()> {
    if device_ids.len() == 0 {
        return OclError::err_string("ocl::core::program_build_err(): \
            Device list is empty. Aborting.");
    }

    for device_id in device_ids.iter().cloned() {
        match get_program_build_info(program, device_id, ProgramBuildInfo::BuildLog) {
            ProgramBuildInfoResult::BuildLog(log) => {
                if log.len() > 1 {
                    let log_readable = format!(
                        "\n\n\
                        ###################### OPENCL PROGRAM BUILD DEBUG OUTPUT \
                        ######################\
                        \n\n{}\n\
                        ########################################################\
                        #######################\
                        \n\n",
                        log);

                    return OclError::err_string(log_readable);
                }
            },
            ProgramBuildInfoResult::Error(err) => return Err(*err),
            _ => panic!("ocl::core::program_build_err(): \
                Unexpected 'ProgramBuildInfoResult' variant."),
        }
    }

    Ok(())
}

/// Verifies that OpenCL versions are above a specified threshold.
pub fn verify_versions(versions: &[OpenclVersion], required_version: [u16; 2]) -> OclResult<()> {
    let reqd_ver = OpenclVersion::from(required_version);

    for dev_v in versions {
        if dev_v < &reqd_ver {
            return OclError::err_string(format!("OpenCL version too low to use this feature \
                (detected: {}, required: {}).", dev_v, reqd_ver));
        }
    }

    Ok(())
}

// Verifies that a platform version (`provided_version`) is above a threshold
// (`required_version`).
fn verify_platform_version<V: ClVersions>(provided_version: Option<&OpenclVersion>,
            required_version: [u16; 2], fallback_version_source: &V) -> OclResult<()> {
    match provided_version {
        Some(pv) => {
            let vers = [pv.clone()];
            verify_versions(&vers, required_version)
        },
        None => fallback_version_source.verify_platform_version(required_version),
    }
}

// Verifies that a device version (`provided_version`) is above a threshold
// (`required_version`).
fn verify_device_version<V: ClVersions>(provided_version: Option<&OpenclVersion>,
            required_version: [u16; 2], fallback_version_source: &V) -> OclResult<()> {
    match provided_version {
        Some(pv) => {
            let ver = [pv.clone()];
            verify_versions(&ver, required_version)
        },
        None => fallback_version_source.verify_device_versions(required_version),
    }
}

// Verifies multiple device versions.
fn verify_device_versions<V: ClVersions>(provided_versions: Option<&[OpenclVersion]>,
            required_version: [u16; 2], fallback_versions_source: &V) -> OclResult<()> {
    match provided_versions {
        Some(pv) => verify_versions(pv, required_version),
        None => fallback_versions_source.verify_device_versions(required_version),
    }
}

//============================================================================
//============================================================================
//======================= OPENCL FUNCTION WRAPPERS ===========================
//============================================================================
//============================================================================

//============================================================================
//============================= Platform API =================================
//============================================================================

/// Returns a list of available platforms as 'core' objects.
pub fn get_platform_ids() -> OclResult<Vec<PlatformId>> {
    let mut num_platforms = 0 as cl_uint;

    // Get a count of available platforms:
    let mut errcode: cl_int = unsafe {
        ffi::clGetPlatformIDs(0, ptr::null_mut(), &mut num_platforms)
    };

    // Deal with ICD wake up problems when called from multiple threads at the
    // same time by adding a delay/retry loop:
    if errcode == Status::CL_PLATFORM_NOT_FOUND_KHR as i32 {
        // println!("CL_PLATFORM_NOT_FOUND_KHR... looping until platform list is available...");
        let sleep_ms = 2000;
        let mut iters_rmng = 5;

        while errcode == Status::CL_PLATFORM_NOT_FOUND_KHR as i32 {
            if iters_rmng == 0 {
                return OclError::err_string(format!("core::get_platform_ids(): \
                    CL_PLATFORM_NOT_FOUND_KHR... Unable to get platform id list after {} \
                    seconds of waiting.", (iters_rmng * sleep_ms) / 1000));
            }

            // Sleep to allow the ICD to refresh or whatever it does:
            thread::sleep(Duration::from_millis(sleep_ms));

            // Get a count of available platforms:
            errcode = unsafe {
                ffi::clGetPlatformIDs(0, ptr::null_mut(), &mut num_platforms)
            };

            iters_rmng -= 1;
        }
    }

    try!(eval_errcode(errcode, (), "clGetPlatformIDs", ""));

    // If no platforms are found, return an empty vec directly:
    if num_platforms == 0 {
        return Ok(vec![]);
    }

    // Create a vec with the appropriate size:
    let mut null_vec: Vec<usize> = iter::repeat(0).take(num_platforms as usize).collect();
    let (ptr, len, cap) = (null_vec.as_mut_ptr(), null_vec.len(), null_vec.capacity());

    // Steal the vec's soul:
    let mut platforms: Vec<PlatformId> = unsafe {
        mem::forget(null_vec);
        Vec::from_raw_parts(ptr as *mut PlatformId, len, cap)
    };

    errcode = unsafe {
        ffi::clGetPlatformIDs(
            num_platforms,
            platforms.as_mut_ptr() as *mut cl_platform_id,
            ptr::null_mut()
        )
    };

    eval_errcode(errcode, platforms, "clGetPlatformIDs", "")
}

/// Returns platform information of the requested type.
pub fn get_platform_info<P: ClPlatformIdPtr>(platform: P, request: PlatformInfo,
        ) -> PlatformInfoResult
{
    let mut result_size = 0 as size_t;

    let errcode = unsafe {
        ffi::clGetPlatformInfo(
            platform.as_ptr(),
            request as cl_platform_info,
            0 as size_t,
            ptr::null_mut(),
            &mut result_size as *mut size_t,
        )
    };

    if let Err(err) = eval_errcode(errcode, (), "clGetPlatformInfo", "") {
        return PlatformInfoResult::Error(Box::new(err));
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return PlatformInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(32u8).take(result_size as usize).collect();

    let errcode = unsafe {
        ffi::clGetPlatformInfo(
            platform.as_ptr(),
            request as cl_platform_info,
            result_size as size_t,
            result.as_mut_ptr() as *mut c_void,
            ptr::null_mut() as *mut size_t,
        )
    };

    let result = eval_errcode(errcode, result, "clGetPlatformInfo", "");
    PlatformInfoResult::from_bytes(request, result)
}

//============================================================================
//============================= Device APIs  =================================
//============================================================================

/// Returns a list of available devices for a particular platform.
pub fn get_device_ids<P: ClPlatformIdPtr>(
            platform: P,
            device_types: Option<DeviceType>,
            devices_max: Option<u32>,
        ) -> OclResult<Vec<DeviceId>>
{
    let device_types = device_types.unwrap_or(try!(default_device_type()));
    let mut devices_available: cl_uint = 0;

    let devices_max = match devices_max {
        Some(d) => {
            if d == 0 {
                return OclError::err_string("ocl::core::get_device_ids(): \
                    `devices_max` can not be zero.");
            } else {
                d
            }
        },
        None => ::DEVICES_MAX,
    };

    let mut device_ids: Vec<DeviceId> = iter::repeat(unsafe { DeviceId::null() } )
        .take(devices_max as usize).collect();

    let errcode = unsafe { ffi::clGetDeviceIDs(
        platform.as_ptr(),
        device_types.bits() as cl_device_type,
        devices_max,
        device_ids.as_mut_ptr() as *mut cl_device_id,
        &mut devices_available,
    ) };
    try!(eval_errcode(errcode, (), "clGetDeviceIDs", ""));

    // Trim vec len:
    unsafe { device_ids.set_len(devices_available as usize); }
    device_ids.shrink_to_fit();

    Ok(device_ids)
}

/// Returns information about a device.
pub fn get_device_info<D: ClDeviceIdPtr>(device: D, request: DeviceInfo)
        -> DeviceInfoResult
{
    let mut result_size: size_t = 0;

    let errcode = unsafe { ffi::clGetDeviceInfo(
        device.as_ptr() as cl_device_id,
        request as cl_device_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // Don't generate a full error report for `CL_INVALID_VALUE` or
    // `CL_INVALID_OPERATION` it's always just an extension unsupported by the
    // device (i.e. `CL_DEVICE_HALF_FP_CONFIG` on Intel or Apple). Note:
    // `CL_INVALID_OPERATION` is actually an invalid error value for this
    // function and is a bug. Don't hold your breath for a fix.
    if errcode < 0 {
        if Status::from_i32(errcode).unwrap() == Status::CL_INVALID_VALUE {
            return OclError::from("<unavailable (CL_INVALID_VALUE)>").into();
        } else if Status::from_i32(errcode).unwrap() == Status::CL_INVALID_OPERATION {
            return OclError::from("<unavailable (CL_INVALID_OPERATION)>").into();
        }
    }

    if let Err(err) = eval_errcode(errcode, (), "clGetDeviceInfo", "") {
        return err.into();
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return DeviceInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetDeviceInfo(
        device.as_ptr() as cl_device_id,
        request as cl_device_info,
        result_size as size_t,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    let result = eval_errcode(errcode, result, "clGetDeviceInfo", "");

    match request {
        DeviceInfo::MaxWorkItemSizes => {
            let max_wi_dims = match get_device_info(device, DeviceInfo::MaxWorkItemDimensions) {
                DeviceInfoResult::MaxWorkItemDimensions(d) => d,
                DeviceInfoResult::Error(err) => return DeviceInfoResult::Error(err),
                _ => panic!("get_device_info(): Error determining dimensions for \
                    'DeviceInfo::MaxWorkItemSizes' due to mismatched variants."),
            };
            DeviceInfoResult::from_bytes_max_work_item_sizes(request, result, max_wi_dims)
        },
        _ => DeviceInfoResult::from_bytes(request, result)
    }
}

/// [UNIMPLEMENTED]
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub fn create_sub_devices(device_version: Option<&OpenclVersion>) -> OclResult<()> {
    // clCreateSubDevices(in_device: cl_device_id,
    //                    properties: *const cl_device_partition_property,
    //                    num_devices: cl_uint,
    //                    out_devices: *mut cl_device_id,
    //                    num_devices_ret: *mut cl_uint) -> cl_int;

    let _ = device_version;
    unimplemented!();
}

/// Increments the reference count of a device.
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub unsafe fn retain_device(device: &DeviceId, device_version: Option<&OpenclVersion>)
            -> OclResult<()> {
    try!(verify_device_version(device_version, [1, 2], device));
    eval_errcode(ffi::clRetainDevice(device.as_ptr()), (), "clRetainDevice", "")
}

/// Decrements the reference count of a device.
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub unsafe fn release_device(device: &DeviceId, device_version: Option<&OpenclVersion>)
            -> OclResult<()> {
    try!(verify_device_version(device_version, [1, 2], device));
    eval_errcode(ffi::clReleaseDevice(device.as_ptr()), (), "clReleaseDevice", "")
}

//============================================================================
//============================= Context APIs  ================================
//============================================================================

/// Creates a new context pointer valid for all devices in `device_ids`.
///
/// Platform is specified in `properties`. If `properties` is `None`, the platform may
/// default to the first available.
///
/// [FIXME]: Incomplete implementation. Callback and userdata untested.
/// [FIXME]: Verify OpenCL Version on property.
/// [FIXME]: Most context sources not implemented for `ContextProperties`.
//
// [NOTE]: Leave commented "DEBUG" print statements intact until more
// `ContextProperties` variants are implemented. [PROBABLY DONE]
pub fn create_context<D: ClDeviceIdPtr>(properties: Option<&ContextProperties>, device_ids: &[D],
            pfn_notify: Option<CreateContextCallbackFn>, user_data: Option<UserDataPtr>
        ) -> OclResult<Context>
{
    if device_ids.len() == 0 {
        return OclError::err_string("ocl::core::create_context(): No devices specified.");
    }

    // [DEBUG]:
    // println!("CREATE_CONTEXT: ORIGINAL: properties: {:?}", properties);

    // https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clCreateContext.html
    // http://sa10.idav.ucdavis.edu/docs/sa10-dg-opencl-gl-interop.pdf
    if let Some(properties) = properties {
        if let Some(_) = properties.get_cgl_sharegroup() {
            for &device in device_ids {
                match device_support_cl_gl_sharing(device) {
                    Ok(true) => {},
                    Ok(false) => return OclError::err_string("A device doesn't support \
                        cl_gl_sharing extension."),
                    Err(err) => return Err(err),
                }
            }
        }
    }

    let properties_bytes: Vec<isize> = match properties {
        Some(props) => props.to_raw(),
        None => Vec::<isize>::with_capacity(0),
    };

    // [DEBUG]:
    // print!("CREATE_CONTEXT: BYTES: ");
    // util::print_bytes_as_hex(&properties_bytes);
    // print!("\n");

    // [FIXME]: Disabled:
    let properties_ptr = if properties_bytes.is_empty() {
        ptr::null() as *const cl_context_properties
    } else {
        properties_bytes.as_ptr()
        // ptr::null() as *const cl_context_properties
    };

    // [FIXME]: Disabled:
    let user_data_ptr = match user_data {
        // Some(ud_ptr) => ud_ptr,
        Some(_) => ptr::null_mut(),
        None => ptr::null_mut(),
    };

    let mut errcode: cl_int = 0;

    let context_ptr = unsafe { ffi::clCreateContext(
        properties_ptr,
        device_ids.len() as cl_uint,
        device_ids.as_ptr()  as *const cl_device_id,
        pfn_notify,
        user_data_ptr,
        &mut errcode,
    ) };
    // [DEBUG]:
    // println!("CREATE_CONTEXT: CONTEXT PTR: {:?}", context);
    eval_errcode(errcode, context_ptr, "clCreateContext", "")
        .map(|ctx_ptr| unsafe { Context::from_raw_create_ptr(ctx_ptr) })
}


/// Creates a new context pointer for all devices of a specific type.
///
/// Platform is specified in `properties`. If `properties` is `None`, the platform may
/// default to the first available.
///
/// [FIXME]: Incomplete implementation. Callback and userdata untested.
/// [FIXME]: Verify OpenCL Version on property.
/// [FIXME]: Most context sources not implemented for `ContextProperties`.
//
// [NOTE]: Leave commented "DEBUG" print statements intact until more
// `ContextProperties` variants are implemented.
pub fn create_context_from_type<D: ClDeviceIdPtr>(properties: Option<&ContextProperties>,
            device_type: DeviceType, pfn_notify: Option<CreateContextCallbackFn>,
            user_data: Option<UserDataPtr>) -> OclResult<Context> {

    // [DEBUG]:
    // println!("CREATE_CONTEXT: ORIGINAL: properties: {:?}", properties);

    // if let &Some(properties) = properties {
    //     if let Some(_) = properties.get_cgl_sharegroup() {
    //         for device in device_ids {
    //             match device_support_cl_gl_sharing(device) {
    //                 Ok(true) => {},
    //                 Ok(false) => return OclError::err_string("A device doesn't support cl_gl_sharing extension."),
    //                 Err(err) => return Err(err),
    //             }
    //         }
    //     }
    // }

    let properties_bytes: Vec<isize> = match properties {
        Some(props) => props.to_raw(),
        None => Vec::<isize>::with_capacity(0),
    };

    // [DEBUG]:
    // print!("CREATE_CONTEXT: BYTES: ");
    // util::print_bytes_as_hex(&properties_bytes);
    // print!("\n");

    // [FIXME]: Disabled:
    let properties_ptr = if properties_bytes.is_empty() {
        ptr::null() as *const cl_context_properties
    } else {
        properties_bytes.as_ptr()
        // ptr::null() as *const cl_context_properties
    };

    // [FIXME]: Disabled:
    let user_data_ptr = match user_data {
        // Some(ud_ptr) => ud_ptr,
        Some(_) => ptr::null_mut(),
        None => ptr::null_mut(),
    };

    let mut errcode: cl_int = 0;

    let context_ptr = unsafe { ffi::clCreateContextFromType(
        properties_ptr,
        device_type.bits(),
        pfn_notify,
        user_data_ptr,
        &mut errcode,
    ) };
    eval_errcode(errcode, context_ptr, "clCreateContextFromType", "")
        .map(|ctx_ptr| unsafe { Context::from_raw_create_ptr(ctx_ptr) })
}


/// Increments the reference count of a context.
pub unsafe fn retain_context<C>(context: C) -> OclResult<()>
        where C: ClContextPtr
{
    eval_errcode(ffi::clRetainContext(context.as_ptr()), (), "clRetainContext", "")
}

/// Decrements reference count of a context.
pub unsafe fn release_context<C>(context: C) -> OclResult<()>
        where C: ClContextPtr
{
    eval_errcode(ffi::clReleaseContext(context.as_ptr()), (), "clReleaseContext", "")
}


fn get_context_info_unparsed<C>(context: C, request: ContextInfo)
        -> OclResult<Vec<u8>>
        where C: ClContextPtr
{
   let mut result_size: size_t = 0;

    let errcode = unsafe { ffi::clGetContextInfo(
        context.as_ptr() as cl_context,
        request as cl_context_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut usize,
    ) };

    eval_errcode(errcode, (), "clGetContextInfo", "")?;

    // if let Err(err) = eval_errcode(errcode, (), "clGetContextInfo", "") {
    //     return ContextInfoResult::Error(Box::new(err));
    // }

    // Check for invalid context pointer (a potentially hard to track down bug)
    // using ridiculous and probably platform-specific logic [if the `Devices`
    // variant is passed and we're not in the release config]:
    if !cfg!(release) {
        let err_if_zero_result_size = request as cl_context_info == ffi::CL_CONTEXT_DEVICES;

        if result_size > 10000 || (result_size == 0 && err_if_zero_result_size) {
            // return ContextInfoResult::Error(Box::new(OclError::from("\n\nocl::core::context_info(): \
            //     Possible invalid context detected. \n\
            //     Context info result size is either '> 10k bytes' or '== 0'. Almost certainly an \n\
            //     invalid context object. If not, please file an issue at: \n\
            //     https://github.com/cogciprocate/ocl/issues.\n\n")));
            return Err(OclError::from("\n\nocl::core::context_info(): \
                Possible invalid context detected. \n\
                Context info result size is either '> 10k bytes' or '== 0'. Almost certainly an \n\
                invalid context object. If not, please file an issue at: \n\
                https://github.com/cogciprocate/ocl/issues.\n\n"));
        }
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        // return ContextInfoResult::from_bytes(request, Ok(vec![]));
        return Ok(vec![]);
    }

    let mut result: Vec<u8> = iter::repeat(0).take(result_size).collect();

    let errcode = unsafe { ffi::clGetContextInfo(
        context.as_ptr() as cl_context,
        request as cl_context_info,
        result_size as size_t,
        result.as_mut_ptr() as *mut c_void,
        0 as *mut usize,
    ) };

    eval_errcode(errcode, result, "clGetContextInfo", "")
}


/// Returns various kinds of context information.
///
/// [SDK Reference](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clGetContextInfo.html)
///
/// # Errors
///
/// Returns an error result for all the reasons listed in the SDK in addition
/// to an additional error when called with `CL_CONTEXT_DEVICES` as described
/// in in the `verify_context()` documentation below.
pub fn get_context_info<C>(context: C, request: ContextInfo) -> ContextInfoResult
        where C: ClContextPtr
{
    ContextInfoResult::from_bytes(request, get_context_info_unparsed(context, request))
}

/// Returns the platform for a context.
///
/// Errors upon the usual OpenCL errors.
///
/// Returns `None` if the context properties do not specify a platform.
pub fn get_context_platform<C>(context: C) -> OclResult<Option<PlatformId>>
        where C: ClContextPtr
{
    let props_raw_bytes = get_context_info_unparsed(context, ContextInfo::Properties)?;

    let prop = unsafe {
        let props_raw = ::util::bytes_into_vec::<isize>(props_raw_bytes)?;
        ContextProperties::extract_property_from_raw(::ContextProperty::Platform, &props_raw)
    };

    if let Some(::ContextPropertyValue::Platform(plat)) = prop {
        Ok(Some(plat))
    } else {
        Ok(None)
    }
}

//============================================================================
//========================== Command Queue APIs ==============================
//============================================================================

/// Returns a new command queue pointer.
pub fn create_command_queue<C, D>(
            context: C,
            device: D,
            properties: Option<CommandQueueProperties>,
        ) -> OclResult<CommandQueue>
        where C: ClContextPtr, D: ClDeviceIdPtr
{
    // Verify that the context is valid:
    try!(verify_context(context));

    let cmd_queue_props = match properties {
        Some(p) => p.bits,
        None => 0,
    };

    let mut errcode: cl_int = 0;

    let cq_ptr = unsafe { ffi::clCreateCommandQueue(
        context.as_ptr(),
        device.as_ptr(),
        cmd_queue_props,
        &mut errcode
    ) };
    eval_errcode(errcode, cq_ptr, "clCreateCommandQueue", "")
        .map(|cq_ptr| unsafe { CommandQueue::from_raw_create_ptr(cq_ptr) })

}

/// Increments the reference count of a command queue.
pub unsafe fn retain_command_queue(queue: &CommandQueue) -> OclResult<()> {
    eval_errcode(ffi::clRetainCommandQueue(queue.as_ptr()), (), "clRetainCommandQueue", "")
}

/// Decrements the reference count of a command queue.
///
/// [FIXME]: Return result
pub unsafe fn release_command_queue(queue: &CommandQueue) -> OclResult<()> {
    eval_errcode(ffi::clReleaseCommandQueue(queue.as_ptr()), (), "clReleaseCommandQueue", "")
}

/// Returns information about a command queue
pub fn get_command_queue_info(queue: &CommandQueue, request: CommandQueueInfo,
        ) -> CommandQueueInfoResult
{
    let mut result_size: size_t = 0;

    let errcode = unsafe { ffi::clGetCommandQueueInfo(
        queue.as_ptr() as cl_command_queue,
        request as cl_command_queue_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // try!(eval_errcode(errcode, result, "clGetCommandQueueInfo", ""));
    if let Err(err) = eval_errcode(errcode, (), "clGetCommandQueueInfo", "") {
        return CommandQueueInfoResult::Error(Box::new(err));
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return CommandQueueInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetCommandQueueInfo(
        queue.as_ptr() as cl_command_queue,
        request as cl_command_queue_info,
        result_size,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    let result = eval_errcode(errcode, result, "clGetCommandQueueInfo", "");
    CommandQueueInfoResult::from_bytes(request, result)
}

//============================================================================
//========================== Memory Object APIs ==============================
//============================================================================

/// Returns a new buffer pointer with size (bytes): `len` * sizeof(T).
pub unsafe fn create_buffer<C, T>(
            context: C,
            flags: MemFlags,
            len: usize,
            data: Option<&[T]>,
        ) -> OclResult<Mem>
        where C: ClContextPtr, T: OclPrm
{
    // Verify that the context is valid:
    try!(verify_context(context));

    let mut errcode: cl_int = 0;

    let host_ptr = match data {
        Some(d) => {
            if d.len() != len {
                return OclError::err_string("ocl::create_buffer(): Data length mismatch.");
            }
            d.as_ptr() as cl_mem
        },
        None => ptr::null_mut(),
    };

    let buf_ptr = ffi::clCreateBuffer(
        context.as_ptr(),
        flags.bits() as cl_mem_flags,
        len * mem::size_of::<T>(),
        host_ptr,
        &mut errcode,
    );

    // [TODO]: Convert back the return style to this:
    eval_errcode(errcode, buf_ptr, "clCreateBuffer", "")
        .map(|ptr| Mem::from_raw_create_ptr(ptr))
}

/// [UNTESTED]
/// Return a buffer pointer from a `OpenGL` buffer object.
pub unsafe fn create_from_gl_buffer<C>(
            context: C,
            gl_object: cl_GLuint,
            flags: MemFlags
        ) -> OclResult<Mem>
        where C: ClContextPtr
{
    // Verify that the context is valid
    try!(verify_context(context));

    let mut errcode: cl_int = 0;

    let buf_ptr = clCreateFromGLBuffer(
            context.as_ptr(),
            flags.bits() as cl_mem_flags,
            gl_object,
            &mut errcode);

    eval_errcode(errcode, buf_ptr, "clCreateFromGLBuffer", "")
        .map(|ptr| Mem::from_raw_create_ptr(ptr))
}

/// [UNTESTED]
/// Return a renderbuffer pointer from a `OpenGL` renderbuffer object.
pub unsafe fn create_from_gl_renderbuffer<C>(
            context: C,
            renderbuffer: cl_GLuint,
            flags: MemFlags
        ) -> OclResult<Mem>
        where C: ClContextPtr
{
    // Verify that the context is valid
    try!(verify_context(context));

    let mut errcode: cl_int = 0;

    let buf_ptr = clCreateFromGLRenderbuffer(
            context.as_ptr(),
            flags.bits() as cl_mem_flags,
            renderbuffer,
            &mut errcode);

    eval_errcode(errcode, buf_ptr, "clCreateFromGLRenderbuffer", "")
        .map(|ptr| Mem::from_raw_create_ptr(ptr))
}

/// [UNTESTED]
/// Return a texture2D pointer from a `OpenGL` texture2D object.
///
/// [TODO]: If version is < 1.2, automatically use older versions.
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub unsafe fn create_from_gl_texture<C>(
            context: C,
            texture_target: cl_GLenum,
            miplevel: cl_GLint,
            texture: cl_GLuint,
            flags: MemFlags,
            device_versions: Option<&[OpenclVersion]>,
        ) -> OclResult<Mem>
        where C: ClContextPtr
{
    // Verify that the context is valid
    try!(verify_context(context));

    // Verify device versions:
    try!(verify_device_versions(device_versions, [1, 2], &context.as_ptr()));

    // [TODO]: Forward old versions to these:
    // let obj_core = match image_desc.image_depth {
    //     2 => unsafe { try!(core::create_from_gl_texture_2d(
    //                         queue.context_core(),
    //                         texture_target,
    //                         miplevel,
    //                         texture,
    //                         flags)) },
    //     3 => unsafe { try!(core::create_from_gl_texture_3d(
    //                         queue.context_core(),
    //                         texture_target,
    //                         miplevel,
    //                         texture,
    //                         flags)) },
    //     _ => unimplemented!() // FIXME: return an error ? or panic! ?
    // };

    let mut errcode: cl_int = 0;

    let buf_ptr = clCreateFromGLTexture(
            context.as_ptr(),
            flags.bits() as cl_mem_flags,
            texture_target,
            miplevel,
            texture,
            &mut errcode);

    eval_errcode(errcode, buf_ptr, "clCreateFromGLTexture", "")
        .map(|ptr| Mem::from_raw_create_ptr(ptr))
}

/// [UNTESTED] [DEPRICATED]
/// Return a texture2D pointer from a `OpenGL` texture2D object.
pub unsafe fn create_from_gl_texture_2d<C>(
            context: C,
            texture_target: cl_GLenum,
            miplevel: cl_GLint,
            texture: cl_GLuint,
            flags: MemFlags
        ) -> OclResult<Mem>
        where C: ClContextPtr
{
    // Verify that the context is valid
    try!(verify_context(context));

    let mut errcode: cl_int = 0;

    let buf_ptr = clCreateFromGLTexture2D(
            context.as_ptr(),
            flags.bits() as cl_mem_flags,
            texture_target,
            miplevel,
            texture,
            &mut errcode);

    eval_errcode(errcode, buf_ptr, "clCreateFromGLTexture2D", "")
        .map(|ptr| Mem::from_raw_create_ptr(ptr))
}

/// [UNTESTED] [DEPRICATED]
/// Return a texture3D pointer from a `OpenGL` texture3D object.
pub unsafe fn create_from_gl_texture_3d<C>(
            context: C,
            texture_target: cl_GLenum,
            miplevel: cl_GLint,
            texture: cl_GLuint,
            flags: MemFlags
        ) -> OclResult<Mem>
        where C: ClContextPtr
{
    // Verify that the context is valid
    try!(verify_context(context));

    let mut errcode: cl_int = 0;

    let buf_ptr = clCreateFromGLTexture3D(
            context.as_ptr(),
            flags.bits() as cl_mem_flags,
            texture_target,
            miplevel,
            texture,
            &mut errcode);

    eval_errcode(errcode, buf_ptr, "clCreateFromGLTexture3D", "")
        .map(|ptr| Mem::from_raw_create_ptr(ptr))
}

/// Creates a new buffer object (referred to as a sub-buffer object) from an
/// existing buffer object.
///
/// The returned sub-buffer has a number of caveats which can cause undefined
/// behavior.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clCreateSubBuffer.html)
///
pub fn create_sub_buffer<T: OclPrm>(
            buffer: &Mem,
            flags: MemFlags,
            buffer_create_info: &BufferRegion<T>,
        ) -> OclResult<Mem>
{
    let buffer_create_type = BufferCreateType::Region;
    let buffer_create_info_bytes = buffer_create_info.to_bytes();
    let mut errcode = 0i32;

    let sub_buf_ptr = unsafe { ffi::clCreateSubBuffer(
        buffer.as_ptr(),
        flags.bits(),
        buffer_create_type as cl_buffer_create_type,
        &buffer_create_info_bytes as *const _ as *const c_void,
        &mut errcode,
    ) };

    eval_errcode(errcode, sub_buf_ptr, "clCreateSubBuffer", "")
        .map(|ptr| unsafe { Mem::from_raw_create_ptr(ptr) })
}

/// Returns a new image (mem) pointer.
///
/// [TODO]: If version is < 1.2, automatically use older versions.
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub unsafe fn create_image<C, T>(
            context: C,
            flags: MemFlags,
            format: &ImageFormat,
            desc: &ImageDescriptor,
            data: Option<&[T]>,
            device_versions: Option<&[OpenclVersion]>,
        ) -> OclResult<Mem>
        where C: ClContextPtr, T: OclPrm
{
    // Verify that the context is valid:
    try!(verify_context(context));

    // Verify device versions:
    try!(verify_device_versions(device_versions, [1, 2], &context.as_ptr()));

    let mut errcode: cl_int = 0;

    let host_ptr = match data {
        Some(d) => {
            // [FIXME]: CALCULATE CORRECT IMAGE SIZE AND COMPARE WITH FORMAT/DESC
            // assert!(d.len() == len, "ocl::create_image(): Data length mismatch.");
            d.as_ptr() as cl_mem
        },
        None => ptr::null_mut(),
    };

    let image_ptr = ffi::clCreateImage(
        context.as_ptr(),
        flags.bits() as cl_mem_flags,
        &format.to_raw() as *const cl_image_format,
        &desc.to_raw() as *const cl_image_desc,
        host_ptr,
        &mut errcode as *mut cl_int,
    );

    eval_errcode(errcode, image_ptr, "clCreateImage", "")
        .map(|ptr| Mem::from_raw_create_ptr(ptr))
}

/// Increments the reference counter of a mem object.
pub unsafe fn retain_mem_object(mem: &Mem) -> OclResult<()> {
    eval_errcode(ffi::clRetainMemObject(mem.as_ptr()), (), "clRetainMemObject", "")
}

/// Decrements the reference counter of a mem object.
pub unsafe fn release_mem_object(mem: &Mem) -> OclResult<()> {
    eval_errcode(ffi::clReleaseMemObject(mem.as_ptr()), (), "clReleaseMemObject", "")
}

/// Returns a list of supported image formats.
///
/// # Example
///
/// ```text
/// let context = Context::builder().build().unwrap();
///
/// let img_fmts = core::get_supported_image_formats(context,
///    core::MEM_READ_WRITE, core::MemObjectType::Image2d)
/// ```
pub fn get_supported_image_formats<C>(
            context: C,
            flags: MemFlags,
            image_type: MemObjectType,
        ) -> OclResult<Vec<ImageFormatParseResult>>
        where C: ClContextPtr
{
    let mut num_image_formats = 0 as cl_uint;

    let errcode = unsafe { ffi::clGetSupportedImageFormats(
        context.as_ptr(),
        flags.bits() as cl_mem_flags,
        image_type as cl_mem_object_type,
        0 as cl_uint,
        ptr::null_mut() as *mut cl_image_format,
        &mut num_image_formats as *mut cl_uint,
    ) };
    try!(eval_errcode(errcode, (), "clGetSupportedImageFormats", ""));

    // If no formats found, return an empty list directly:
    if num_image_formats == 0 {
        return Ok(vec![]);
    }

    let mut image_formats: Vec<cl_image_format> = (0..(num_image_formats as usize)).map(|_| {
           ImageFormat::new_raw()
        } ).collect();

    debug_assert!(image_formats.len() == num_image_formats as usize && image_formats.len() > 0);

    let errcode = unsafe { ffi::clGetSupportedImageFormats(
        context.as_ptr(),
        flags.bits() as cl_mem_flags,
        image_type as cl_mem_object_type,
        num_image_formats,
        image_formats.as_mut_ptr() as *mut _ as *mut cl_image_format,
        0 as *mut cl_uint,
    ) };

    try!(eval_errcode(errcode, (), "clGetSupportedImageFormats", ""));
    Ok(ImageFormat::list_from_raw(image_formats))
}


/// Get mem object info.
pub fn get_mem_object_info(obj: &Mem, request: MemInfo) -> MemInfoResult {
    let mut result_size: size_t = 0;

    let errcode = unsafe { ffi::clGetMemObjectInfo(
        obj.as_ptr() as cl_mem,
        request as cl_mem_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // try!(eval_errcode(errcode, result, "clGetMemObjectInfo", ""));
    if let Err(err) = eval_errcode(errcode, (), "clGetMemObjectInfo", "") {
        return MemInfoResult::Error(Box::new(err));
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return MemInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetMemObjectInfo(
        obj.as_ptr() as cl_mem,
        request as cl_mem_info,
        result_size,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };
    let result = eval_errcode(errcode, result, "clGetMemObjectInfo", "");
    MemInfoResult::from_bytes(request, result)
}


/// Get image info.
pub fn get_image_info(obj: &Mem, request: ImageInfo) -> ImageInfoResult {
    let mut result_size: size_t = 0;

    let errcode = unsafe { ffi::clGetImageInfo(
        obj.as_ptr() as cl_mem,
        request as cl_image_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // try!(eval_errcode(errcode, result, "clGetImageInfo", ""));
    if let Err(err) = eval_errcode(errcode, (), "clGetImageInfo", "") {
        return ImageInfoResult::Error(Box::new(err));
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return ImageInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetImageInfo(
        obj.as_ptr() as cl_mem,
        request as cl_image_info,
        result_size,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    let result = eval_errcode(errcode, result, "clGetImageInfo", "");
    ImageInfoResult::from_bytes(request, result)
}

/// [UNIMPLEMENTED]
pub fn set_mem_object_destructor_callback() -> OclResult<()> {
    // ffi::clSetMemObjectDestructorCallback(memobj: cl_mem,
    //                                     pfn_notify: extern fn (cl_mem, *mut c_void),
    //                                     user_data: *mut c_void) -> cl_int;
    unimplemented!();
}

//============================================================================
//============================= Sampler APIs =================================
//============================================================================

/// Creates and returns a new sampler object.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clCreateSampler.html)
pub fn create_sampler<C>(context: C, normalize_coords: bool, addressing_mode: AddressingMode,
            filter_mode: FilterMode) -> OclResult<Sampler>
        where C: ClContextPtr
{
    let mut errcode = 0;

    let sampler = unsafe { Sampler::from_raw_create_ptr(ffi::clCreateSampler(
        context.as_ptr(),
        normalize_coords as cl_bool,
        addressing_mode as cl_addressing_mode,
        filter_mode as cl_filter_mode,
        &mut errcode,
    )) };

    eval_errcode(errcode, sampler, "clCreateSampler", "")
}

/// Increments a sampler reference counter.
pub unsafe fn retain_sampler(sampler: &Sampler) -> OclResult<()> {
    eval_errcode(ffi::clRetainSampler(sampler.as_ptr()), (), "clRetainSampler", "")
}

/// Decrements a sampler reference counter.
pub unsafe fn release_sampler(sampler: &Sampler) -> OclResult<()> {
    eval_errcode(ffi::clReleaseSampler(sampler.as_ptr()), (), "clReleaseSampler", "")
}

/// Returns information about the sampler object.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clGetSamplerInfo.html)
pub fn get_sampler_info(obj: &Sampler, request: SamplerInfo,
    ) -> SamplerInfoResult
{
    let mut result_size: size_t = 0;

    let errcode = unsafe { ffi::clGetSamplerInfo(
        obj.as_ptr() as cl_sampler,
        request as cl_sampler_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // try!(eval_errcode(errcode, result, "clGetSamplerInfo", ""));
    if let Err(err) = eval_errcode(errcode, (), "clGetSamplerInfo", "") {
        return SamplerInfoResult::Error(Box::new(err));
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return SamplerInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetSamplerInfo(
        obj.as_ptr() as cl_sampler,
        request as cl_sampler_info,
        result_size,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    let result = eval_errcode(errcode, result, "clGetSamplerInfo", "");
    SamplerInfoResult::from_bytes(request, result)
}

//============================================================================
//========================== Program Object APIs =============================
//============================================================================

/// Creates a new program.
pub fn create_program_with_source<C>(
            context: C,
            src_strings: &[CString],
        ) -> OclResult<Program>
        where C: ClContextPtr
{
    // Verify that the context is valid:
    try!(verify_context(context));

    // Lengths (not including \0 terminator) of each string:
    let ks_lens: Vec<usize> = src_strings.iter().map(|cs| cs.as_bytes().len()).collect();

    // Pointers to each string:
    let kern_string_ptrs: Vec<*const _> = src_strings.iter().map(|cs| cs.as_ptr()).collect();

    let mut errcode: cl_int = 0;

    let program_ptr = unsafe { ffi::clCreateProgramWithSource(
        context.as_ptr(),
        kern_string_ptrs.len() as cl_uint,
        kern_string_ptrs.as_ptr() as *const *const _,
        ks_lens.as_ptr() as *const usize,
        &mut errcode,
    ) };

    eval_errcode(errcode, program_ptr, "clCreateProgramWithSource", "")
        .map(|ptr| unsafe { Program::from_raw_create_ptr(ptr) })
}

/// [UNTESTED]
/// Creates a program object for a context, and loads the binary bits
/// specified by binary into the program object.
///
/// [SDK Docs]: https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clCreateProgramWithBinary.html
///
pub fn create_program_with_binary<C, D>(
            context: C,
            devices: &[D],
            binaries: &[&[u8]],
        ) -> OclResult<Program>
        where C: ClContextPtr, D: ClDeviceIdPtr
{
    // assert!(devices.len() > 0);
    // assert!(devices.len() == binaries.len());
    if devices.len() == 0 { return OclError::err_string("ocl::create_program_with_binary: \
        Length of 'devices' must be greater than zero."); }
    if devices.len() != binaries.len() { return OclError::err_string("ocl::create_program_with_binary: \
        Length of 'devices' must equal the length of 'binaries' (e.g. one binary per device)."); }

    let lengths: Vec<usize> = binaries.iter().map(|bin| bin.len()).collect();
    let mut binary_status: Vec<i32> = iter::repeat(0).take(devices.len()).collect();
    let mut errcode: cl_int = 0;

    let program = unsafe { ffi::clCreateProgramWithBinary(
        context.as_ptr(),
        devices.len() as u32,
        devices.as_ptr() as *const _ as *const cl_device_id,
        lengths.as_ptr(),
        binaries.as_ptr() as *const *const u8,
        binary_status.as_mut_ptr(),
        &mut errcode,
    ) };

    try!(eval_errcode(errcode, (), "clCreateProgramWithBinary", ""));

    for (i, item) in binary_status.iter().enumerate() {
        try!(eval_errcode(*item, (), "clCreateProgramWithBinary", &format!("(): Device [{}]", i)));
    }

    unsafe { Ok(Program::from_raw_create_ptr(program)) }
}

/// [UNIMPLEMENTED]
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub fn create_program_with_built_in_kernels(device_version: Option<&OpenclVersion>)
            -> OclResult<()> {
    // clCreateProgramWithBuiltInKernels(context: cl_context,
    //                                  num_devices: cl_uint,
    //                                  device_list: *const cl_device_id,
    //                                  kernel_names: *mut char,
    //                                  errcode_ret: *mut cl_int) -> cl_program;
    let _ =  device_version;
    unimplemented!();
}

/// Returns a new `Program` loaded with the provided IL bytes.
///
/// [Version Controlled: OpenCL 2.1+] See module docs for more info.
#[cfg(feature = "opencl_version_2_1")]
pub fn create_program_with_il<C>(
        context: C,
        il: &[u8],
        device_versions: Option<&[OpenclVersion]>,
        ) -> OclResult<Program>
        where C: ClContextPtr
{
    verify_device_versions(device_versions, [2, 1], &context)?;

    let mut errcode: cl_int = 0;

    let program_ptr = unsafe { ffi::clCreateProgramWithIL(
        context.as_ptr(),
        il.as_ptr() as *mut c_void,
        il.len(),
        &mut errcode,
    ) };

    eval_errcode(errcode, program_ptr, "clCreateProgramWithIL", "")
        .map(|ptr| unsafe { Program::from_raw_create_ptr(ptr) })
}

/// Increments a program reference counter.
pub unsafe fn retain_program(program: &Program) -> OclResult<()> {
    eval_errcode(ffi::clRetainProgram(program.as_ptr()), (), "clRetainProgram", "")
}

/// Decrements a program reference counter.
pub unsafe fn release_program(program: &Program) -> OclResult<()> {
    eval_errcode(ffi::clReleaseProgram(program.as_ptr()), (), "clReleaseKernel", "")
}

pub struct UserDataPh(usize);

impl UserDataPh {
    fn unwrapped(&self) -> *mut c_void {
        ptr::null_mut()
    }
}

/// Builds a program.
///
/// Callback functions are not yet supported. Please file an issue if you have
/// need of this functionality.
///
//
// [NOTE]: Despite what the spec says, some platforms segfault when `null` is
// passed for `devices_ptr`.
pub fn build_program<D: ClDeviceIdPtr>(
            program: &Program,
            devices: Option<&[D]>,
            options: &CString,
            pfn_notify: Option<BuildProgramCallbackFn>,
            user_data: Option<Box<UserDataPh>>,
        ) -> OclResult<()>
{
    assert!(pfn_notify.is_none() && user_data.is_none(),
        "ocl::core::build_program(): Callback functions not yet implemented.");

    let (devices_len, devices_ptr) = match devices {
        Some(dvs) => (dvs.len() as u32, dvs.as_ptr() as *const cl_device_id),
        None => (0, ptr::null() as *const cl_device_id),
    };

    let user_data = match user_data {
        Some(ud) => ud.unwrapped(),
        None => ptr::null_mut(),
    };

    let errcode = unsafe { ffi::clBuildProgram(
        program.as_ptr() as cl_program,
        devices_len,
        devices_ptr,
        options.as_ptr(),
        pfn_notify,
        user_data,
    ) };

    if errcode == Status::CL_BUILD_PROGRAM_FAILURE as i32 {
        if let Some(ds) = devices {
            program_build_err(program, ds)
        } else {
            let ds = program.devices()?;
            program_build_err(program, &ds)
        }
    } else {
        eval_errcode(errcode, (), "clBuildProgram", "")
    }
}

/// [UNIMPLEMENTED]
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub fn compile_program(device_version: Option<&OpenclVersion>) -> OclResult<()> {
    // clCompileProgram(program: cl_program,
    //                 num_devices: cl_uint,
    //                 device_list: *const cl_device_id,
    //                 options: *const c_char,
    //                 num_input_headers: cl_uint,
    //                 input_headers: *const cl_program,
    //                 header_include_names: *const *const c_char,
    //                 pfn_notify: extern fn (program: cl_program, user_data: *mut c_void),
    //                 user_data: *mut c_void) -> cl_int;
    let _ = device_version;
    unimplemented!();
}

/// [UNIMPLEMENTED]
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub fn link_program(device_version: Option<&OpenclVersion>) -> OclResult<()> {
    // clLinkProgram(context: cl_context,
    //               num_devices: cl_uint,
    //               device_list: *const cl_device_id,
    //               options: *const c_char,
    //               num_input_programs: cl_uint,
    //               input_programs: *const cl_program,
    //               pfn_notify: extern fn (program: cl_program, user_data: *mut c_void),
    //               user_data: *mut c_void,
    //               errcode_ret: *mut cl_int) -> cl_program;
    let _ = device_version;
    unimplemented!();
}

// [DISABLED DUE TO PLATFORM INCOMPATABILITY]
// /// [UNTESTED]
// /// Unloads a platform compiler.
// ///
// /// [Version Controlled: OpenCL 1.2+] See module docs for more info.
// pub fn unload_platform_compiler(platform: &PlatformId,
//          device_version: Option<&OpenclVersion>) -> OclResult<()> {
//     unsafe { eval_errcode("clUnloadPlatformCompiler", "",
//         ffi::clUnloadPlatformCompiler(platform.as_ptr())) }
// }

/// Get program info.
pub fn get_program_info(obj: &Program, request: ProgramInfo) -> ProgramInfoResult {
    let mut result_size: size_t = 0;

    let errcode = unsafe { ffi::clGetProgramInfo(
        obj.as_ptr() as cl_program,
        request as cl_program_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // try!(eval_errcode(errcode, result, "clGetProgramInfo", ""));
    if let Err(err) = eval_errcode(errcode, (), "clGetProgramInfo", "") {
        return ProgramInfoResult::Error(Box::new(err));
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return ProgramInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetProgramInfo(
        obj.as_ptr() as cl_program,
        request as cl_program_info,
        result_size,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    let result = eval_errcode(errcode, result, "clGetProgramInfo", "");
    ProgramInfoResult::from_bytes(request, result)
}

/// Get program build info.
pub fn get_program_build_info<D: ClDeviceIdPtr + Debug>(obj: &Program, device_obj: D,
            request: ProgramBuildInfo) -> ProgramBuildInfoResult
{
    let mut result_size: size_t = 0;

    // println!("ocl::core::get_program_build_info(): device_obj: {:?}", device_obj);

    let errcode = unsafe { ffi::clGetProgramBuildInfo(
        obj.as_ptr() as cl_program,
        device_obj.as_ptr() as cl_device_id,
        request as cl_program_build_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // try!(eval_errcode(errcode, result, "clGetProgramBuildInfo", ""));
    if let Err(err) = eval_errcode(errcode, (), "clGetProgramBuildInfo", "") {
        return ProgramBuildInfoResult::Error(Box::new(err));
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return ProgramBuildInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetProgramBuildInfo(
        obj.as_ptr() as cl_program,
        device_obj.as_ptr() as cl_device_id,
        request as cl_program_build_info,
        result_size as size_t,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    let result = eval_errcode(errcode, result, "clGetProgramBuildInfo", "");
    ProgramBuildInfoResult::from_bytes(request, result)
}

//============================================================================
//========================== Kernel Object APIs ==============================
//============================================================================

/// Returns a new kernel.
pub fn create_kernel(program: &Program, name: &str) -> OclResult<Kernel> {
    let mut err: cl_int = 0;

    unsafe {
        let kernel_ptr = ffi::clCreateKernel(
            program.as_ptr(),
            // 0 as cl_program,
            try!(CString::new(name.as_bytes())).as_ptr(),
            &mut err,
        );

        eval_errcode(err, kernel_ptr, "clCreateKernel", name)
            .map(|ptr| Kernel::from_raw_create_ptr(ptr))
    }
}

/// [UNIMPLEMENTED]
pub fn create_kernels_in_program() -> OclResult<()> {
    // ffi::clCreateKernelsInProgram(program: cl_program,
    //                             num_kernels: cl_uint,
    //                             kernels: *mut cl_kernel,
    //                             num_kernels_ret: *mut cl_uint) -> cl_int;
    unimplemented!();
}

/// Increments a kernel reference counter.
pub unsafe fn retain_kernel(kernel: &Kernel) -> OclResult<()> {
    eval_errcode(ffi::clRetainKernel(kernel.as_ptr()), (), "clRetainKernel", "")
}

/// Decrements a kernel reference counter.
pub unsafe fn release_kernel(kernel: &Kernel) -> OclResult<()> {
    eval_errcode(ffi::clReleaseKernel(kernel.as_ptr()), (), "clReleaseKernel", "")
}


/// Sets the argument value for a specific argument of a kernel.
///
/// [SDK Documentation](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clSetKernelArg.html)
///
/// [FIXME: Remove] `kernel_name` is for error reporting and is optional but highly recommended.
///
/// TODO: Remove `name` parameter and lookup name with `get_kernel_info` instead.
pub fn set_kernel_arg<T: OclPrm>(kernel: &Kernel, arg_index: u32, arg: KernelArg<T>,
        ) -> OclResult<()>
{
    // [DEBUG] LEAVE THIS HERE:
        // println!("SET_KERNEL_ARG: KERNELARG: {:?}", arg);
    // [/DEBUG]

    let (arg_size, arg_value): (size_t, *const c_void) = match arg {
        KernelArg::Mem(mem_core_ref) => (
            mem::size_of::<cl_mem>() as size_t,
            mem_core_ref as *const _ as *const c_void
        ),
        KernelArg::Sampler(smplr_core_ref) => (
            mem::size_of::<cl_sampler>() as size_t,
            smplr_core_ref as *const _ as *const c_void
        ),
        KernelArg::Scalar(ref scalar) => (
            mem::size_of::<T>() as size_t,
            scalar as *const T as *const c_void
        ),
        KernelArg::Vector(ref scalar) => (
            mem::size_of::<T>() as size_t,
            scalar as *const T as *const c_void
        ),
        KernelArg::Local(length) => (
            (mem::size_of::<T>() * length) as size_t,
            ptr::null()
        ),
        KernelArg::UnsafePointer { size, value } => (size, value),
        _ => (mem::size_of::<*const c_void>() as size_t, ptr::null()),
    };

    // [DEBUG] LEAVE THIS HERE:
        // println!("SET_KERNEL_ARG: KERNEL: {:?}", kernel);
        // println!("SET_KERNEL_ARG: index: {:?}", arg_index);
        // println!("SET_KERNEL_ARG: size: {:?}", arg_size);
        // println!("SET_KERNEL_ARG: value: {:?}", arg_value);
        // println!("SET_KERNEL_ARG: name: {:?}", name);
        // print!("\n");
    // [/DEBUG]

    let err = unsafe { ffi::clSetKernelArg(
            kernel.as_ptr(),
            arg_index,
            arg_size,
            arg_value,
    ) };

    if err != Status::CL_SUCCESS as i32 {
        let name = get_kernel_name(kernel);
        eval_errcode(err, (), "clSetKernelArg", &name)
    } else {
        Ok(())
    }
}

/// Get kernel info.
pub fn get_kernel_info(obj: &Kernel, request: KernelInfo) -> KernelInfoResult {
    let mut result_size: size_t = 0;

    let errcode = unsafe { ffi::clGetKernelInfo(
        obj.as_ptr() as cl_kernel,
        request as cl_kernel_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // try!(eval_errcode(errcode, result, "clGetKernelInfo", ""));
    if let Err(err) = eval_errcode(errcode, (), "clGetKernelInfo", "") {
        return KernelInfoResult::Error(Box::new(err));
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return KernelInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetKernelInfo(
        obj.as_ptr() as cl_kernel,
        request as cl_kernel_info,
        result_size,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    let result = eval_errcode(errcode, result, "clGetKernelInfo", "");
    KernelInfoResult::from_bytes(request, result)
}

/// Get kernel arg info.
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub fn get_kernel_arg_info(obj: &Kernel, arg_index: u32, request: KernelArgInfo,
        device_versions: Option<&[OpenclVersion]>) -> KernelArgInfoResult
{
    // Verify device version:
    if let Err(err) = verify_device_versions(device_versions, [1, 2], obj) {
        return KernelArgInfoResult::from(err)
    }

    let mut result_size: size_t = 0;

    let errcode = unsafe { ffi::clGetKernelArgInfo(
        obj.as_ptr() as cl_kernel,
        arg_index as cl_uint,
        request as cl_kernel_arg_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // try!(eval_errcode(errcode, result, "clGetKernelArgInfo", ""));
    if let Err(err) = eval_errcode(errcode, (), "clGetKernelArgInfo", "") {
        return KernelArgInfoResult::from(err);
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return KernelArgInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetKernelArgInfo(
        obj.as_ptr() as cl_kernel,
        arg_index as cl_uint,
        request as cl_kernel_arg_info,
        result_size,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    let result = eval_errcode(errcode, result, "clGetKernelArgInfo", "");
    KernelArgInfoResult::from_bytes(request, result)
}

/// Get kernel work group info.
pub fn get_kernel_work_group_info<D: ClDeviceIdPtr>(obj: &Kernel, device_obj: D,
            request: KernelWorkGroupInfo) -> KernelWorkGroupInfoResult
{
    let mut result_size: size_t = 0;

    let errcode = unsafe { ffi::clGetKernelWorkGroupInfo(
        obj.as_ptr() as cl_kernel,
        device_obj.as_ptr() as cl_device_id,
        request as cl_kernel_work_group_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // Make printing certain platform-specific errors less scary looking:
    if let Err(err) = eval_errcode(errcode, (), "clGetKernelWorkGroupInfo", "") {
        if let OclError::Status { ref status, .. } = err {
            // NVIDIA / APPLE (i think):
            if request == KernelWorkGroupInfo::GlobalWorkSize &&
                    status == &Status::CL_INVALID_VALUE
            {
                // return KernelWorkGroupInfoResult::from(OclError::from(
                //     "only available for custom devices or built-in kernels"));
                return KernelWorkGroupInfoResult::CustomBuiltinOnly;
            }

            // APPLE (bleh):
            if status == &Status::CL_INVALID_DEVICE {
                return KernelWorkGroupInfoResult::Unavailable(status.clone());
            }
        }
        return KernelWorkGroupInfoResult::from(err);
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return KernelWorkGroupInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetKernelWorkGroupInfo(
        obj.as_ptr() as cl_kernel,
        device_obj.as_ptr() as cl_device_id,
        request as cl_kernel_work_group_info,
        result_size,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    let result = eval_errcode(errcode, result, "clGetKernelWorkGroupInfo", "");
    KernelWorkGroupInfoResult::from_bytes(request, result)
}

//============================================================================
//========================== Event Object APIs ===============================
//============================================================================

/// Blocks until the first `num_events` events in `event_list` are complete.
pub fn wait_for_events(num_events: u32, event_list: &ClWaitListPtr) -> OclResult<()> {
    assert!(event_list.count() >= num_events);

    let errcode = unsafe {
        ffi::clWaitForEvents(num_events, event_list.as_ptr_ptr())
    };

    eval_errcode(errcode, (), "clWaitForEvents", "")
}

/// Get event info.
pub fn get_event_info<'e, E: ClEventPtrRef<'e>>(event: &'e E, request: EventInfo) -> EventInfoResult {
    let mut result_size: size_t = 0;

    let errcode = unsafe { ffi::clGetEventInfo(
        *event.as_ptr_ref(),
        request as cl_event_info,
        0 as size_t,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // try!(eval_errcode(errcode, result, "clGetEventInfo", ""));
    if let Err(err) = eval_errcode(errcode, (), "clGetEventInfo", "") {
        return EventInfoResult::Error(Box::new(err));
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return EventInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetEventInfo(
        *event.as_ptr_ref(),
        request as cl_event_info,
        result_size,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    let result = eval_errcode(errcode, result, "clGetEventInfo", "");
    EventInfoResult::from_bytes(request, result)
}

/// Creates an event not already associated with any command.
pub fn create_user_event<C>(context: C) -> OclResult<Event>
        where C: ClContextPtr
{
    let mut errcode = 0;
    let event = unsafe { Event::from_raw_create_ptr(ffi::clCreateUserEvent(context.as_ptr(), &mut errcode)) };
    eval_errcode(errcode, event, "clCreateUserEvent", "")
}

/// Increments an event's reference counter.
pub unsafe fn retain_event<'e, E: ClEventPtrRef<'e>>(event: &'e E) -> OclResult<()> {
    eval_errcode(ffi::clRetainEvent(*event.as_ptr_ref()), (), "clRetainEvent", "")
}

/// Decrements an event's reference counter.
pub unsafe fn release_event<'e, E: ClEventPtrRef<'e>>(event: &'e E) -> OclResult<()> {
    eval_errcode(ffi::clReleaseEvent(*event.as_ptr_ref()), (), "clReleaseEvent", "")
}

/// Updates a user events status.
///
/// Setting status to completion will cause commands waiting upon this event
/// to execute.
///
/// Will return an error if the event is not a 'user' event (created with
/// `::create_user_event`).
///
/// Valid options are (for OpenCL versions 1.1 - 2.1):
///
/// `CommandExecutionStatus::Complete`
/// `CommandExecutionStatus::Running`
/// `CommandExecutionStatus::Submitted`
/// `CommandExecutionStatus::Queued`
///
/// To the best of the author's knowledge, the only variant that matters is
/// `::Complete`. Everything else is functionally equivalent and is useful
/// only for debugging or profiling purposes (this may change in the future).
///
pub fn set_user_event_status<'e,E: ClEventPtrRef<'e>>(event: &'e E,
            execution_status: CommandExecutionStatus) -> OclResult<()>
{
    unsafe {
        #[cfg(feature = "event_debug_print")]
        println!("::set_user_event_status: Setting user event status for event: {:?}", *event.as_ptr_ref());

        eval_errcode(ffi::clSetUserEventStatus(*event.as_ptr_ref(), execution_status as cl_int),
            (), "clSetUserEventStatus", ""
        )
    }
}

/// Sets a callback function which is called as soon as the `callback_trigger`
/// status is reached.
pub unsafe fn set_event_callback<'e, E: ClEventPtrRef<'e>>(
            event: &'e E,
            callback_trigger: CommandExecutionStatus,
            callback_receiver: Option<EventCallbackFn>,
            user_data: *mut c_void,
        ) -> OclResult<()>
{
    eval_errcode(ffi::clSetEventCallback(
        *event.as_ptr_ref(),
        callback_trigger as cl_int,
        callback_receiver,
        user_data,
    ), (), "clSetEventCallback", "")
}

//============================================================================
//============================ Profiling APIs ================================
//============================================================================

/// Get event profiling info (for debugging / benchmarking).
pub fn get_event_profiling_info<'e, E: ClEventPtrRef<'e>>(event: &'e E, request: ProfilingInfo,
        ) -> ProfilingInfoResult
{
    // Apple compatibile value:
    let max_result_size_bytes = 8;
    let mut result_size: size_t = 0;
    let event: cl_event = unsafe { *event.as_ptr_ref() };

    let errcode = unsafe { ffi::clGetEventProfilingInfo(
        event,
        request as cl_profiling_info,
        max_result_size_bytes,
        0 as *mut c_void,
        &mut result_size as *mut size_t,
    ) };

    // Make sure our assumption about the maximum value was correct:
    assert!(result_size <= max_result_size_bytes);

    // Don't generate a full error report for `CL_INVALID_VALUE` it just means
    // that event profiling info is not available on this platform.
    if errcode < 0 {
        if Status::from_i32(errcode).unwrap() == Status::CL_INVALID_VALUE {
            return OclError::from("<unavailable (CL_INVALID_VALUE)>").into();
        }
    }

    // try!(eval_errcode(errcode, result, "clGetEventProfilingInfo", ""));
    if let Err(err) = eval_errcode(errcode, (), "clGetEventProfilingInfo", "") {
        return ProfilingInfoResult::Error(Box::new(err));
    }

    // If result size is zero, return an empty info result directly:
    if result_size == 0 {
        return ProfilingInfoResult::from_bytes(request, Ok(vec![]));
    }

    let mut result: Vec<u8> = iter::repeat(0u8).take(result_size).collect();

    let errcode = unsafe { ffi::clGetEventProfilingInfo(
        event,
        request as cl_profiling_info,
        result_size,
        result.as_mut_ptr() as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    let result = eval_errcode(errcode, result, "clGetEventProfilingInfo", "");
    ProfilingInfoResult::from_bytes(request, result)
}

//============================================================================
//========================= Flush and Finish APIs ============================
//============================================================================

/// Flushes a command queue.
///
/// Issues all previously queued OpenCL commands in a command-queue to the
/// device associated with the command-queue.
pub fn flush(command_queue: &CommandQueue) -> OclResult<()> {
    unsafe { eval_errcode(ffi::clFlush(command_queue.as_ptr()), (), "clFlush", "") }
}

/// Waits for a queue to finish.
///
/// Blocks until all previously queued OpenCL commands in a command-queue are
/// issued to the associated device and have completed.
pub fn finish(command_queue: &CommandQueue) -> OclResult<()> {
    unsafe {
        let errcode = ffi::clFinish(command_queue.as_ptr());
        eval_errcode(errcode, (), "clFinish", "")
    }
}

//============================================================================
//======================= Enqueued Commands APIs =============================
//============================================================================

/// Enqueues a read from device memory referred to by `buffer` to device memory,
/// `data`.
///
/// ## Safety
///
/// Caller must ensure that `data` lives until the read is complete. Use
/// `new_event` to monitor it (use [`core::EventList::last_clone`] if passing
/// an event list as `new_event`).
///
///
/// [`core::EventList::get_clone`]: struct.EventList.html#method.last_clone
///
pub unsafe fn enqueue_read_buffer<T, M, En, Ewl>(
        command_queue: &CommandQueue,
        buffer: M,
        block: bool,
        offset: usize,
        data: &mut [T],
        wait_list: Option<Ewl>,
        new_event: Option<En>,
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdRw
{
    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let offset_bytes = offset * mem::size_of::<T>();

    let errcode = ffi::clEnqueueReadBuffer(
        command_queue.as_ptr(),
        buffer.as_mem().as_ptr(),
        block as cl_uint,
        offset_bytes,
        (data.len() * mem::size_of::<T>()) as size_t,
        data.as_ptr() as cl_mem,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    );

    eval_errcode(errcode, (), "clEnqueueReadBuffer", "")
}

/// Enqueues a command to read from a rectangular region from a buffer object to host memory.
///
/// ## Safety
///
/// Caller must ensure that `data` lives until the read is complete. Use
/// `new_event` to monitor it (use [`core::EventList::last_clone`] if passing
/// an event list as `new_event`).
///
/// ## Official Documentation
///
/// [SDK - clEnqueueReadBufferRect](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueReadBufferRect.html)
///
///
/// [`core::EventList::get_clone`]: struct.EventList.html#method.last_clone
///
pub unsafe fn enqueue_read_buffer_rect<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            buffer: M,
            block: bool,
            buffer_origin: [usize; 3],
            host_origin: [usize; 3],
            region: [usize; 3],
            buffer_row_pitch_bytes: usize,
            buffer_slc_pitch_bytes: usize,
            host_row_pitch_bytes: usize,
            host_slc_pitch_bytes: usize,
            data: &mut [T],
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdRw
{
    let buffer_origin_bytes = [buffer_origin[0] * mem::size_of::<T>(),
        buffer_origin[1], buffer_origin[2]];
    let host_origin_bytes = [host_origin[0] * mem::size_of::<T>(),
        host_origin[1], host_origin[2]];
    let region_bytes = [region[0] * mem::size_of::<T>(), region[1], region[2]];
    // let buffer_row_pitch_bytes = buffer_row_pitch * mem::size_of::<T>();
    // let buffer_slc_pitch_bytes = buffer_slc_pitch * mem::size_of::<T>();
    // let host_row_pitch_bytes = host_row_pitch * mem::size_of::<T>();
    // let host_slc_pitch_bytes = host_slc_pitch * mem::size_of::<T>();

    // DEBUG:
    if false {
        println!("buffer_origin_bytes: {:?}, host_origin_bytes: {:?}, region_bytes: {:?}",
            buffer_origin_bytes, host_origin_bytes, region_bytes);
        println!("buffer_row_pitch_bytes: {}, buffer_slc_pitch_bytes: {}, \
            host_row_pitch_bytes: {}, host_slc_pitch_bytes: {}",
            buffer_row_pitch_bytes, buffer_slc_pitch_bytes, host_row_pitch_bytes, host_slc_pitch_bytes);
    }

    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let errcode = ffi::clEnqueueReadBufferRect(
        command_queue.as_ptr(),
        buffer.as_mem().as_ptr(),
        block as cl_uint,
        &buffer_origin_bytes as *const _ as *const usize,
        &host_origin_bytes as *const _ as *const usize,
        &region_bytes as *const _ as *const usize,
        buffer_row_pitch_bytes,
        buffer_slc_pitch_bytes,
        host_row_pitch_bytes,
        host_slc_pitch_bytes,
        data.as_ptr() as cl_mem,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    );

    eval_errcode(errcode, (), "clEnqueueReadBufferRect", "")
}

/// Enqueues a write from host memory, `data`, to device memory referred to by
/// `buffer`.
pub fn enqueue_write_buffer<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            buffer: M,
            block: bool,
            offset: usize,
            data: &[T],
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdRw
{
    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let offset_bytes = offset * mem::size_of::<T>();

    let errcode = unsafe { ffi::clEnqueueWriteBuffer(
        command_queue.as_ptr(),
        buffer.as_mem().as_ptr(),
        block as cl_uint,
        offset_bytes,
        (data.len() * mem::size_of::<T>()) as size_t,
        data.as_ptr() as cl_mem,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueWriteBuffer", "")
}

/// Enqueues a command to write from a rectangular region from host memory to a buffer object.
///
/// ## Official Documentation
///
/// [SDK - clEnqueueWriteBufferRect]
///
/// [SDK - clEnqueueWriteBufferRect]: https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueWriteBufferRect.html
///
pub fn enqueue_write_buffer_rect<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            buffer: M,
            block: bool,
            buffer_origin: [usize; 3],
            host_origin: [usize; 3],
            region: [usize; 3],
            buffer_row_pitch_bytes: usize,
            buffer_slc_pitch_bytes: usize,
            host_row_pitch_bytes: usize,
            host_slc_pitch_bytes: usize,
            data: &[T],
            wait_list: Option<Ewl>,
            new_event: Option<En>,
    ) -> OclResult<()>
    where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdRw
{
    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let buffer_origin_bytes = [buffer_origin[0] * mem::size_of::<T>(),
        buffer_origin[1], buffer_origin[2]];
    let host_origin_bytes = [host_origin[0] * mem::size_of::<T>(),
        host_origin[1], host_origin[2]];
    let region_bytes = [region[0] * mem::size_of::<T>(), region[1], region[2]];
    // let buffer_row_pitch_bytes = buffer_row_pitch * mem::size_of::<T>();
    // let buffer_slc_pitch_bytes = buffer_slc_pitch * mem::size_of::<T>();
    // let host_row_pitch_bytes = host_row_pitch * mem::size_of::<T>();
    // let host_slc_pitch_bytes = host_slc_pitch * mem::size_of::<T>();

    let errcode = unsafe { ffi::clEnqueueWriteBufferRect(
        command_queue.as_ptr(),
        buffer.as_mem().as_ptr(),
        block as cl_uint,
        &buffer_origin_bytes as *const _ as *const usize,
        &host_origin_bytes as *const _ as *const usize,
        &region_bytes as *const _ as *const usize,
        buffer_row_pitch_bytes,
        buffer_slc_pitch_bytes,
        host_row_pitch_bytes,
        host_slc_pitch_bytes,
        data.as_ptr() as cl_mem,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueWriteBufferRect", "")
}

/// Enqueues a command to fill a buffer object with a pattern of a given pattern size.
///
/// ## Pattern (from [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueFillBuffer.html))
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub fn enqueue_fill_buffer<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            buffer: M,
            pattern: T,
            offset: usize,
            len: usize,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
            device_version: Option<&OpenclVersion>
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdRw
{
    try!(verify_device_version(device_version, [1, 2], command_queue));

    let pattern_size = mem::size_of::<T>();
    let offset_bytes = offset * mem::size_of::<T>();
    let size_bytes = len * mem::size_of::<T>();

    let (wait_list_len, wait_list_ptr, new_event_ptr)
        = resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { ffi::clEnqueueFillBuffer(
        command_queue.as_ptr(),
        buffer.as_mem().as_ptr(),
        &pattern as *const _ as *const c_void,
        pattern_size,
        offset_bytes,
        size_bytes,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueFillBuffer", "")
}

/// Copies the contents of one buffer to another.
pub fn enqueue_copy_buffer<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            src_buffer: M,
            dst_buffer: M,
            src_offset: usize,
            dst_offset: usize,
            len: usize,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdAll
{
    let (wait_list_len, wait_list_ptr, new_event_ptr)
        = resolve_event_ptrs(wait_list, new_event);

    let src_offset_bytes = src_offset * mem::size_of::<T>();
    let dst_offset_bytes = dst_offset * mem::size_of::<T>();
    let len_bytes = len * mem::size_of::<T>();

    let errcode = unsafe { ffi::clEnqueueCopyBuffer(
        command_queue.as_ptr(),
        src_buffer.as_mem().as_ptr(),
        dst_buffer.as_mem().as_ptr(),
        src_offset_bytes,
        dst_offset_bytes,
        len_bytes,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueCopyBuffer", "")
}

/// Enqueues a command to copy a rectangular region from a buffer object to
/// another buffer object.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueCopyBufferRect.html)
///
pub fn enqueue_copy_buffer_rect<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            src_buffer: M,
            dst_buffer: M,
            src_origin: [usize; 3],
            dst_origin: [usize; 3],
            region: [usize; 3],
            src_row_pitch_bytes: usize,
            src_slc_pitch_bytes: usize,
            dst_row_pitch_bytes: usize,
            dst_slc_pitch_bytes: usize,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdAll
{
    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let src_origin_bytes = [src_origin[0] * mem::size_of::<T>(),
        src_origin[1], src_origin[2]];
    let dst_origin_bytes = [dst_origin[0] * mem::size_of::<T>(),
        dst_origin[1], dst_origin[2]];
    let region_bytes = [region[0] * mem::size_of::<T>(), region[1], region[2]];
    // let src_row_pitch_bytes = src_row_pitch * mem::size_of::<T>();
    // let src_slc_pitch_bytes = src_slc_pitch * mem::size_of::<T>();
    // let dst_row_pitch_bytes = dst_row_pitch * mem::size_of::<T>();
    // let dst_slc_pitch_bytes = dst_slc_pitch * mem::size_of::<T>();

    let errcode = unsafe { ffi::clEnqueueCopyBufferRect(
        command_queue.as_ptr(),
        src_buffer.as_mem().as_ptr(),
        dst_buffer.as_mem().as_ptr(),
        &src_origin_bytes as *const _ as *const usize,
        &dst_origin_bytes as *const _ as *const usize,
        &region_bytes as *const _ as *const usize,
        src_row_pitch_bytes,
        src_slc_pitch_bytes,
        dst_row_pitch_bytes,
        dst_slc_pitch_bytes,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueCopyBufferRect", "")
}

/// [UNTESTED]
/// Enqueue acquire OpenCL memory objects that have been created from `OpenGL` objects.
pub fn enqueue_acquire_gl_buffer<En, Ewl>(
            command_queue: &CommandQueue,
            buffer: &Mem,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where En: ClNullEventPtr, Ewl: ClWaitListPtr
{
    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { clEnqueueAcquireGLObjects(
        command_queue.as_ptr(),
        1,
        &buffer.as_ptr(),
        wait_list_len,
        wait_list_ptr,
        new_event_ptr
    ) };
    eval_errcode(errcode, (), "clEnqueueAcquireGLObjects", "")
}

/// [UNTESTED]
/// Enqueue release OpenCL memory objects that have been created from `OpenGL` objects.
pub fn enqueue_release_gl_buffer<En, Ewl>(
            command_queue: &CommandQueue,
            buffer: &Mem,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where En: ClNullEventPtr, Ewl: ClWaitListPtr
{
    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { clEnqueueReleaseGLObjects(
        command_queue.as_ptr(),
        1,
        &buffer.as_ptr(),
        wait_list_len,
        wait_list_ptr,
        new_event_ptr
    ) };
    eval_errcode(errcode, (), "clEnqueueReleaseGLObjects", "")
}


/// Reads an image from device to host memory.
///
/// ## Safety
///
/// Caller must ensure that `data` lives until the read is complete. Use
/// `new_event` to monitor it (use [`core::EventList::last_clone`] if passing
/// an event list as `new_event`).
///
/// [`core::EventList::get_clone`]: struct.EventList.html#method.last_clone
///
// pub unsafe fn enqueue_read_image<T>(
pub unsafe fn enqueue_read_image<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            image: M,
            block: bool,
            origin: [usize; 3],
            region: [usize; 3],
            row_pitch_bytes: usize,
            slc_pitch_bytes: usize,
            data: &mut [T],
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdRw
{
    // let row_pitch_bytes = row_pitch * mem::size_of::<T>();
    // let slc_pitch_bytes = slc_pitch * mem::size_of::<T>();

    let (wait_list_len, wait_list_ptr, new_event_ptr)
        = resolve_event_ptrs(wait_list, new_event);

    let errcode = ffi::clEnqueueReadImage(
        command_queue.as_ptr(),
        image.as_mem().as_ptr(),
        block as cl_uint,
        &origin as *const _ as *const usize,
        &region as *const _ as *const usize,
        row_pitch_bytes,
        slc_pitch_bytes,
        data.as_ptr() as cl_mem,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    );
    eval_errcode(errcode, (), "clEnqueueReadImage", "")
}


/// Enqueues a command to write to an image or image array object from host memory.
///
/// TODO: Size check (rather than leaving it to API).
pub fn enqueue_write_image<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            image: M,
            block: bool,
            origin: [usize; 3],
            region: [usize; 3],
            input_row_pitch_bytes: usize,
            input_slc_pitch_bytes: usize,
            data: &[T],
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdRw
{
    // let input_row_pitch_bytes = input_row_pitch * mem::size_of::<T>();
    // let input_slc_pitch_bytes = input_slc_pitch * mem::size_of::<T>();

    let (wait_list_len, wait_list_ptr, new_event_ptr)
        = resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { ffi::clEnqueueWriteImage(
        command_queue.as_ptr(),
        image.as_mem().as_ptr(),
        block as cl_uint,
        &origin as *const _ as *const usize,
        &region as *const _ as *const usize,
        input_row_pitch_bytes,
        input_slc_pitch_bytes,
        data.as_ptr() as cl_mem,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueWriteImage", "")
}

/// [UNTESTED]
/// Enqueues a command to fill an image object with a specified color.
///
/// ## Fill Color (from [SDK docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueFillImage.html)
///
/// The fill color. The fill color is a four component RGBA floating-point color
/// value if the image channel data type is not an unnormalized signed and
/// unsigned integer type, is a four component signed integer value if the image
/// channel data type is an unnormalized signed integer type and is a four
/// component unsigned integer value if the image channel data type is an
/// unormalized unsigned integer type. The fill color will be converted to the
/// appropriate image channel format and order associated with image.
///
/// TODO: Trait constraints for `T`. Presumably it should be 32bits? Testing needed.
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub fn enqueue_fill_image<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            image: M,
            color: &[T],
            origin: [usize; 3],
            region: [usize; 3],
            wait_list: Option<Ewl>,
            new_event: Option<En>,
            device_version: Option<&OpenclVersion>
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdAll
{
    // Verify device version:
    try!(verify_device_version(device_version, [1, 2], command_queue));

    let (wait_list_len, wait_list_ptr, new_event_ptr)
        = resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { ffi::clEnqueueFillImage(
        command_queue.as_ptr(),
        image.as_mem().as_ptr(),
        color as *const _ as *const c_void,
        &origin as *const _ as *const usize,
        &region as *const _ as *const usize,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueFillImage", "")
}


/// Enqueues a command to copy image objects.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueCopyImage.html)
pub fn enqueue_copy_image<En, Ewl>(
            command_queue: &CommandQueue,
            src_image: &Mem,
            dst_image: &Mem,
            src_origin: [usize; 3],
            dst_origin: [usize; 3],
            region: [usize; 3],
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where En: ClNullEventPtr, Ewl: ClWaitListPtr
{
    let (wait_list_len, wait_list_ptr, new_event_ptr)
        = resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { ffi::clEnqueueCopyImage(
        command_queue.as_ptr(),
        src_image.as_ptr(),
        dst_image.as_ptr(),
        &src_origin as *const _ as *const usize,
        &dst_origin as *const _ as *const usize,
        &region as *const _ as *const usize,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueCopyImage", "")
}

/// [UNTESTED]
/// Enqueues a command to copy an image object to a buffer object.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueCopyImageToBuffer.html)
pub fn enqueue_copy_image_to_buffer<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            src_image: M,
            dst_buffer: M,
            src_origin: [usize; 3],
            region: [usize; 3],
            dst_offset: usize,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdAll
{
    let dst_offset_bytes = dst_offset * mem::size_of::<T>();

    let (wait_list_len, wait_list_ptr, new_event_ptr)
        = resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { ffi::clEnqueueCopyImageToBuffer(
        command_queue.as_ptr(),
        src_image.as_mem().as_ptr(),
        dst_buffer.as_mem().as_ptr(),
        &src_origin as *const _ as *const usize,
        &region as *const _ as *const usize,
        dst_offset_bytes,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueCopyImageToBuffer", "")
}

/// [UNTESTED]
/// Enqueues a command to copy a buffer object to an image object.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueCopyBufferToImage.html)
pub fn enqueue_copy_buffer_to_image<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            src_buffer: M,
            dst_image: M,
            src_offset: usize,
            dst_origin: [usize; 3],
            region: [usize; 3],
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdAll
{
    let src_offset_bytes = src_offset * mem::size_of::<T>();

    let (wait_list_len, wait_list_ptr, new_event_ptr)
        = resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { ffi::clEnqueueCopyBufferToImage(
        command_queue.as_ptr(),
        src_buffer.as_mem().as_ptr(),
        dst_image.as_mem().as_ptr(),
        src_offset_bytes,
        &dst_origin as *const _ as *const usize,
        &region as *const _ as *const usize,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueCopyBufferToImage", "")
}

#[inline]
unsafe fn _enqueue_map_buffer<T, M>(
        command_queue: &CommandQueue,
        buffer: M,
        block: bool,
        map_flags: MapFlags,
        offset: usize,
        len: usize,
        wait_list_len: cl_uint,
        wait_list_ptr: *const cl_event,
        new_event_ptr: *mut cl_event,
        ) -> OclResult<*mut T>
        where T: OclPrm, M: AsMem<T> + MemCmdAll
{
    let offset_bytes = offset * mem::size_of::<T>();
    let size_bytes = len * mem::size_of::<T>();

    let mut errcode = 0i32;

    let mapped_ptr = ffi::clEnqueueMapBuffer(
        command_queue.as_ptr(),
        buffer.as_mem().as_ptr(),
        block as cl_uint,
        map_flags.bits(),
        offset_bytes,
        size_bytes,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
        &mut errcode,
    );

    eval_errcode(errcode, mapped_ptr as *mut T, "clEnqueueMapBuffer", "")
}

/// Enqueues a command to map a region of the buffer object given
/// by `buffer` into the host address space and returns a pointer to this
/// mapped region.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueMapBuffer.html)
///
/// ## Safety
///
/// Caller must ensure that the returned pointer is not used until the map is
/// complete. Use `new_event` to monitor it. [TEMPORARY] It also must be
/// ensured that memory referred to by the returned pointer is not dropped,
/// reused, or otherwise interfered with until `enqueue_unmap_mem_object` is
/// called.
///
///
/// TODO: Return a new wrapped type representing the newly mapped memory.
///
/// [`EventList::get_clone`]: struct.EventList.html#method.last_clone
///
///
pub unsafe fn enqueue_map_buffer<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            buffer: M,
            block: bool,
            map_flags: MapFlags,
            offset: usize,
            len: usize,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<MemMap<T>>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdAll
{
    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let mapped_ptr_res = _enqueue_map_buffer(command_queue, buffer.as_mem(), block, map_flags, offset, len,
        wait_list_len, wait_list_ptr, new_event_ptr);

    // mapped_ptr_res.map(|ptr| MemMap::new(ptr as *mut T, len, None, buffer.as_mem().clone(),
    //     command_queue.clone()))

    // eval_errcode(errcode, mapped_ptr, "clEnqueueMapImage", "")
    //     .map(|ptr| MemMap::from_raw(ptr as *mut _ as *mut T))
    mapped_ptr_res.map(|ptr| MemMap::from_raw(ptr))
}

/// [UNTESTED]
/// Enqueues a command to map a region of the image object given by `image` into
/// the host address space and returns a pointer to this mapped region.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueMapBuffer.html)
///
/// ## Safety
///
/// Caller must ensure that the returned pointer is not used until the map is
/// complete. Use `new_event` to monitor it. [TEMPORARY] It also must be
/// ensured that memory referred to by the returned pointer is not dropped,
/// reused, or otherwise interfered with until `enqueue_unmap_mem_object` is
/// called.
///
///
/// TODO: Return a new wrapped type representing the newly mapped memory.
///
/// [`EventList::get_clone`]: struct.EventList.html#method.last_clone
///
///
pub unsafe fn enqueue_map_image<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            image: M,
            block: bool,
            map_flags: MapFlags,
            origin: [usize; 3],
            region: [usize; 3],
            row_pitch_bytes: &mut usize,
            slc_pitch_bytes: &mut usize,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<MemMap<T>>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdAll
{
    // let row_pitch_bytes = row_pitch * mem::size_of::<T>();
    // let slc_pitch_bytes = slc_pitch * mem::size_of::<T>();

    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let mut errcode = 0i32;
    // let mut map_event = 0 as cl_event;

    let mapped_ptr = ffi::clEnqueueMapImage(
        command_queue.as_ptr(),
        image.as_mem().as_ptr(),
        block as cl_uint,
        map_flags.bits(),
        &origin as *const _ as *const usize,
        &region as *const _ as *const usize,
        row_pitch_bytes,
        slc_pitch_bytes,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
        // &mut map_event,
        &mut errcode,
    );

    // let map_event_core = if !new_event_ptr.is_null() {
    //     *new_event_ptr = map_event;
    //     Event::from_raw_copied_ptr(map_event)?
    // } else {
    //     Event::from_raw_create_ptr(map_event)
    // };

    // eval_errcode(errcode, MemMap::new(mapped_ptr as *mut T, slc_pitch * region[2],
    //     None, image.as_mem().clone(), command_queue.clone()), "clEnqueueMapImage", "")

    eval_errcode(errcode, mapped_ptr, "clEnqueueMapImage", "")
        .map(|ptr| MemMap::from_raw(ptr as *mut _ as *mut T))
}

/// Enqueues a command to unmap a previously mapped region of a memory object.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueUnmapMemObject.html)
///
pub fn enqueue_unmap_mem_object<T, M, En, Ewl>(
            command_queue: &CommandQueue,
            memobj: M,
            mapped_mem: &MemMap<T>,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
        ) -> OclResult<()>
        where T: OclPrm, En: ClNullEventPtr, Ewl: ClWaitListPtr, M: AsMem<T> + MemCmdAll
{

    // if mapped_mem.is_unmapped() {
    //     return Err("ocl_core::enqueue_unmap_mem_object: The 'MemMap' object passed is already \
    //         unmapped.".into());
    // }

    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { ffi::clEnqueueUnmapMemObject(
        command_queue.as_ptr(),
        memobj.as_mem().as_ptr(),
        mapped_mem.as_void_ptr(),
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };

    eval_errcode(errcode, (), "clEnqueueUnmapMemObject", "")
}

/// [UNTESTED]
/// Enqueues a command to indicate which device a set of memory objects should
/// be associated with.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueMigrateMemObjects.html)
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub fn enqueue_migrate_mem_objects<En: ClNullEventPtr, Ewl: ClWaitListPtr>(
            command_queue: &CommandQueue,
            // num_mem_objects: u32,
            mem_objects: &[Mem],
            flags: MemMigrationFlags,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
            device_version: Option<&OpenclVersion>
        ) -> OclResult<()>
{
    // Verify device version:
    try!(verify_device_version(device_version, [1, 2], command_queue));

    let (wait_list_len, wait_list_ptr, new_event_ptr)
        = resolve_event_ptrs(wait_list, new_event);

    // let mem_ptr_list: Vec<cl_mem> = mem_objects.iter()
    //     .map(|ref mem_obj| mem_obj.as_ptr()).collect();

    let errcode = unsafe { ffi::clEnqueueMigrateMemObjects(
        command_queue.as_ptr(),
        // num_mem_objects,
        mem_objects.len() as u32,
        // mem_ptr_list.as_ptr() as *const _ as *const cl_mem,
        mem_objects.as_ptr() as *const _ as *const cl_mem,
        flags.bits(),
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueMigrateMemObjects", "")
}

/// Enqueues a command to execute a kernel on a device.
///
/// # Safety
///
/// Running any kernel is an inherently unsafe process. As an API call this
/// function is safe. The kernel code it may run must always be considered
/// unsafe.
///
/// # Stability
///
/// * Work dimension/offset sizes *may* eventually be wrapped up in
///   specialized types.
/// * This function may (and probably should) be marked unsafe to reflect the
///   inherent of kernel code.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueNDRangeKernel.html)
//
// FIXME: Mark this unsafe
//
pub fn enqueue_kernel<En: ClNullEventPtr, Ewl: ClWaitListPtr> (
            command_queue: &CommandQueue,
            kernel: &Kernel,
            work_dims: u32,
            global_work_offset: Option<[usize; 3]>,
            global_work_dims: &[usize; 3],
            local_work_dims: Option<[usize; 3]>,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
            // kernel_name: Option<&str>
        ) -> OclResult<()>
{
    // if !cfg!(release) {
    //     #[allow(unused_imports)] use std::thread;
    //     #[allow(unused_imports)] use std::time::Duration;
    // }

    #[cfg(feature="kernel_debug_print")]
    println!("Resolving events: wait_list: {:?}, new_event: {:?}", wait_list, new_event);

    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    #[cfg(feature="kernel_debug_print")]
    println!("Resolving global work offset: {:?}...", global_work_offset);

    let gwo = resolve_work_dims(global_work_offset.as_ref());

    #[cfg(feature="kernel_debug_print")]
    println!("Assigning global work size: {:?}...", global_work_dims);

    let gws = global_work_dims as *const size_t;

    #[cfg(feature="kernel_debug_print")]
    println!("Resolving local work size: {:?}...", local_work_dims);

    let lws = resolve_work_dims(local_work_dims.as_ref());

    #[cfg(feature="kernel_debug_print")]
    println!("Preparing to print all details...");

    #[cfg(feature="kernel_debug_print")]
    print!("core::enqueue_kernel('{}': \
        work_dims: {}, \
        gwo: {:?}, \
        gws: {:?}, \
        lws: {:?}, \
        wait_list_len: {}, \
        wait_list_ptr: {:?}, \
        new_event_ptr: {:?}) \
        ",
        get_kernel_name(&kernel),
        work_dims,
        global_work_offset,
        global_work_dims,
        local_work_dims,
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    );

    let errcode = unsafe { ffi::clEnqueueNDRangeKernel(
            command_queue.as_ptr(),
            kernel.as_ptr() as cl_kernel,
            work_dims,
            gwo,
            gws,
            lws,
            wait_list_len,
            wait_list_ptr,
            new_event_ptr,
    ) };

    if cfg!(feature="kernel_debug_print") { println!("-> Status: {}.", errcode); }
    if cfg!(feature="kernel_debug_sleep") {
        thread::sleep(Duration::from_millis(KERNEL_DEBUG_SLEEP_DURATION_MS));
    }

    if errcode != 0 {
        let name = get_kernel_name(kernel);
        eval_errcode(errcode, (), "clEnqueueNDRangeKernel", &name)
    } else {
        Ok(())
    }
}

/// [UNTESTED] Enqueues a command to execute a kernel on a device.
///
/// The kernel is executed using a single work-item.
///
/// From [SDK]: clEnqueueTask is equivalent to calling clEnqueueNDRangeKernel
/// with work_dim = 1, global_work_offset = NULL, global_work_size[0] set to 1,
/// and local_work_size[0] set to 1.
///
/// [SDK]: https://www.khronos.org/registry/cl/sdk/1.0/docs/man/xhtml/clEnqueueTask.html
///
pub fn enqueue_task<En: ClNullEventPtr, Ewl: ClWaitListPtr>(
            command_queue: &CommandQueue,
            kernel: &Kernel,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
            kernel_name: Option<&str>
        ) -> OclResult<()>
{
    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { ffi::clEnqueueTask(
            command_queue.as_ptr(),
            kernel.as_ptr() as cl_kernel,
            wait_list_len,
            wait_list_ptr,
            new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueTask", kernel_name.unwrap_or(""))
}

/// [UNIMPLEMENTED]
pub fn enqueue_native_kernel() -> OclResult<()> {
    // ffi::clEnqueueNativeKernel(command_queue: cl_command_queue,
    //                          user_func: extern fn (*mut c_void),
    //                          args: *mut c_void,
    //                          cb_args: size_t,
    //                          num_mem_objects: cl_uint,
    //                          mem_list: *const cl_mem,
    //                          args_mem_loc: *const *const c_void,
    //                          num_events_in_wait_list: cl_uint,
    //                          event_wait_list: *const cl_event,
    //                          event: *mut cl_event) -> cl_int;
    unimplemented!();
}

/// Enqueues a marker command which waits for either a list of events to
/// complete, or all previously enqueued commands to complete.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueMarkerWithWaitList.html)
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub fn enqueue_marker_with_wait_list<En, Ewl>(
            command_queue: &CommandQueue,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
            device_version: Option<&OpenclVersion>
        ) -> OclResult<()>
        where En: ClNullEventPtr, Ewl: ClWaitListPtr
{
    // Verify device version:
    try!(verify_device_version(device_version, [1, 2], command_queue));

    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { ffi::clEnqueueMarkerWithWaitList(
        command_queue.as_ptr(),
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueMarkerWithWaitList", "")
}

/// A synchronization point that enqueues a barrier operation.
///
/// [SDK Docs](https://www.khronos.org/registry/cl/sdk/1.2/docs/man/xhtml/clEnqueueBarrierWithWaitList.html)
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub fn enqueue_barrier_with_wait_list<En, Ewl>(
            command_queue: &CommandQueue,
            wait_list: Option<Ewl>,
            new_event: Option<En>,
            device_version: Option<&OpenclVersion>
        ) -> OclResult<()>
        where En: ClNullEventPtr, Ewl: ClWaitListPtr
{
    // Verify device version:
    try!(verify_device_version(device_version, [1, 2], command_queue));

    let (wait_list_len, wait_list_ptr, new_event_ptr) =
        resolve_event_ptrs(wait_list, new_event);

    let errcode = unsafe { ffi::clEnqueueBarrierWithWaitList(
        command_queue.as_ptr(),
        wait_list_len,
        wait_list_ptr,
        new_event_ptr,
    ) };
    eval_errcode(errcode, (), "clEnqueueBarrierWithWaitList", "")
}



// [UNTESTED]
// Extension function access
//
// Returns the extension function address for the given function name,
// or NULL if a valid function can not be found. The client must
// check to make sure the address is not NULL, before using or
// or calling the returned function address.
//
// A non-NULL return value for clGetExtensionFunctionAddressForPlatform does
// not guarantee that an extension function is actually supported by the
// platform. The application must also make a corresponding query using
// clGetPlatformInfo (platform, CL_PLATFORM_EXTENSIONS, ... ) or
// clGetDeviceInfo (device,CL_DEVICE_EXTENSIONS, ... ) to determine if an
// extension is supported by the OpenCL implementation.
//
// [FIXME]: Return a generic that implements `Fn` (or `FnMut/Once`?).
// TODO: Create another function which will handle the second check described
// above in addition to calling this.
//
// ////////////////////////////////////////////////////////////////////////////
/// [UNTESTED]
/// Returns the address of the extension function named by
/// `func_name` for a given platform.
///
/// The pointer returned should be cast to a function pointer type matching
/// the extension function's definition defined in the appropriate extension
/// specification and header file.
///
///
/// A non-NULL return value does
/// not guarantee that an extension function is actually supported by the
/// platform. The application must also make a corresponding query using
/// `ocl::core::get_platform_info(platform_core, CL_PLATFORM_EXTENSIONS, ... )` or
/// `ocl::core::get_device_info(device_core, CL_DEVICE_EXTENSIONS, ... )`
/// to determine if an extension is supported by the OpenCL implementation.
///
/// [FIXME]: Update enum names above to the wrapped types.
///
/// # Errors
///
/// Returns an error if:
///
/// - `func_name` cannot be converted to a `CString`.
/// - The specified function does not exist for the implementation.
/// - 'platform' is not a valid platform.
///
/// [Version Controlled: OpenCL 1.2+] See module docs for more info.
pub unsafe fn get_extension_function_address_for_platform(
            platform: &PlatformId,
            func_name: &str,
            platform_version: Option<&OpenclVersion>
        ) -> OclResult<*mut c_void>
{
    // Verify platform version:
    try!(verify_platform_version(platform_version, [1, 2], platform));

    let func_name_c = try!(CString::new(func_name));

    let ext_fn = ffi::clGetExtensionFunctionAddressForPlatform(
        platform.as_ptr(),
        func_name_c.as_ptr(),
    );

    if ext_fn == 0 as *mut c_void {
        OclError::err_string("The specified function does not exist for the implementation or 'platform' \
            is not a valid platform.")
    } else {
        Ok(ext_fn)
    }
}

//============================================================================
//============================================================================
//=========================== DERIVED FUNCTIONS ==============================
//============================================================================
//============================================================================


/// Returns a list of versions for devices.
pub fn device_versions(device_ids: &[DeviceId]) -> OclResult<Vec<OpenclVersion>> {
    let mut d_versions = Vec::with_capacity(device_ids.len());

    for device_id in device_ids {
        d_versions.push(try!(device_id.version()));
    }

    Ok(d_versions)
}

/// Returns the default platform if set by an environment variable or config
/// file.
pub fn default_platform_idx() -> usize {
    match env::var("OCL_DEFAULT_PLATFORM_IDX") {
        Ok(s) => s.parse::<usize>().unwrap_or(0),
        Err(_) => 0,
    }
}

/// Returns the default or first platform.
pub fn default_platform() -> OclResult<PlatformId> {
    let platform_list = try!(get_platform_ids());

    if platform_list.is_empty() {
        OclError::err_string("No platforms found!")
    } else {
        let default_platform_idx = default_platform_idx();
        if default_platform_idx > platform_list.len() - 1 {
            OclError::err_string(format!("The default platform set by the environment variable \
                'OCL_DEFAULT_PLATFORM_IDX' has an index which is out of range \
                (index: [{}], max: [{}]).", default_platform_idx, platform_list.len() - 1))
        } else {
            Ok(platform_list[default_platform_idx])
        }
    }
}

/// Returns the default device type bitflags as specified by environment
/// variable or else `DEVICE_TYPE_ALL`.
pub fn default_device_type() -> OclResult<DeviceType> {
    match env::var("OCL_DEFAULT_DEVICE_TYPE") {
        Ok(ref s) => match s.trim() {
            "DEFAULT" => Ok(::DEVICE_TYPE_DEFAULT),
            "CPU" => Ok(::DEVICE_TYPE_CPU),
            "GPU" => Ok(::DEVICE_TYPE_GPU),
            "ACCELERATOR" => Ok(::DEVICE_TYPE_ACCELERATOR),
            "CUSTOM" => Ok(::DEVICE_TYPE_CUSTOM),
            "ALL" => Ok(::DEVICE_TYPE_ALL),
            _ => OclError::err_string(format!("The default device type set by the environment variable \
                'OCL_DEFAULT_DEVICE_TYPE': ('{}') is invalid. Valid types are: 'DEFAULT', 'CPU', \
                'GPU', 'ACCELERATOR', 'CUSTOM', and 'ALL'.", s)),
        },
        Err(_) => Ok(::DEVICE_TYPE_ALL),
    }
}

/// Returns the name of a kernel.
pub fn get_kernel_name(kernel: &Kernel) -> String {
    let result = get_kernel_info(kernel, KernelInfo::FunctionName);
    result.into()
}

/// Creates, builds, and returns a new program pointer from `src_strings`.
///
/// TODO: Break out create and build parts into requisite functions then call
/// from here.
pub fn create_build_program<C, D>(
            context: C,
            src_strings: &[CString],
            device_ids: Option<&[D]>,
            cmplr_opts: &CString,
        ) -> OclResult<Program>
        where C: ClContextPtr, D: ClDeviceIdPtr + Debug
{
    let program = try!(create_program_with_source(context, src_strings));
    try!(build_program(&program, device_ids, cmplr_opts, None, None));
    Ok(program)
}


#[allow(dead_code)]
/// Blocks until an event is complete.
pub fn wait_for_event<'e, E: ClEventPtrRef<'e>>(event: &'e E) -> OclResult<()> {
    let errcode = unsafe {
        // let event_ptr = *event.as_ptr_ref();
        // ffi::clWaitForEvents(1, &event_ptr)
        ffi::clWaitForEvents(1, event.as_ptr_ref())
    };
    eval_errcode(errcode, (), "clWaitForEvents", "")
}

/// Returns the status of `event`.
pub fn event_status<'e, E: ClEventPtrRef<'e>>(event: &'e E) -> OclResult<CommandExecutionStatus> {
    let mut status_int: cl_int = 0;

    let errcode = unsafe {
        ffi::clGetEventInfo(
            *event.as_ptr_ref(),
            ffi::CL_EVENT_COMMAND_EXECUTION_STATUS,
            mem::size_of::<cl_int>(),
            &mut status_int as *mut _ as *mut c_void,
            ptr::null_mut(),
        )
    };
    try!(eval_errcode(errcode, (), "clGetEventInfo", ""));

    CommandExecutionStatus::from_i32(status_int).ok_or_else(|| OclError::from("Error converting \
        'clGetEventInfo' status output."))
}

/// Returns true if an event is complete, false if not complete.
pub fn event_is_complete<'e, E: ClEventPtrRef<'e>>(event: &'e E) -> OclResult<bool> {
    let mut status_int: cl_int = 0;

    let errcode = unsafe {
        ffi::clGetEventInfo(
            *event.as_ptr_ref(),
            ffi::CL_EVENT_COMMAND_EXECUTION_STATUS,
            mem::size_of::<cl_int>(),
            &mut status_int as *mut _ as *mut c_void,
            ptr::null_mut(),
        )
    };

    #[cfg(feature = "event_debug_print")]
    unsafe {
        println!("Event Status: {:?} (ptr: {:?})",
            CommandExecutionStatus::from_i32(status_int).unwrap(),
            *event.as_ptr_ref());
    }

    eval_errcode(errcode, status_int == CommandExecutionStatus::Complete as i32,
        "clEventGetInfo", "CL_EVENT_COMMAND_EXECUTION_STATUS")
}



/// Verifies that the `context` is in fact a context object pointer.
///
/// # Assumptions
///
/// Some (most?/all?) OpenCL implementations do not correctly error if non-
/// context pointers are passed. This function relies on the fact that passing
/// the `CL_CONTEXT_DEVICES` as the `param_name` to `clGetContextInfo` will
/// (at least on my AMD implementation) often return a huge result size if
/// `context` is not actually a `cl_context` pointer due to the fact that it's
/// reading from some random memory location on non-context structs. Also
/// checks for zero because a context must have at least one device (true?).
/// Should probably choose a value lower than 10kB because it seems unlikely
/// any result would be that big but w/e.
///
/// [UPDATE]: This function may no longer be necessary now that the core
/// pointers have wrappers but it still prevents a hard to track down bug so
/// it will stay intact for now.
///
#[inline]
pub fn verify_context<C>(context: C) -> OclResult<()>
        where C: ClContextPtr
{
    // context_info(context, ffi::CL_CONTEXT_REFERENCE_COUNT)
    if cfg!(release) {
        Ok(())
    } else {
        match get_context_info(context, ContextInfo::Devices) {
            ContextInfoResult::Error(err) => Err(*err),
            _ => Ok(()),
        }
    }
}


/// Checks to see if a device supports the `CL_GL_SHARING_EXT` extension.
//
fn device_support_cl_gl_sharing<D: ClDeviceIdPtr>(device: D) -> OclResult<bool> {
    match get_device_info(device, DeviceInfo::Extensions) {
        DeviceInfoResult::Extensions(extensions) => Ok(extensions.contains(CL_GL_SHARING_EXT)),
        DeviceInfoResult::Error(err) => Err(*err),
        _ => unreachable!(),
    }
}


/// Returns the context for a command queue, bypassing extra processing.
pub fn get_command_queue_context_ptr(queue: &CommandQueue) -> OclResult<cl_context> {
    let mut result = 0 as cl_context;
    let result_size = mem::size_of::<cl_context>();

    let errcode = unsafe { ffi::clGetCommandQueueInfo(
        queue.as_ptr(),
        CommandQueueInfo::Context as cl_command_queue_info,
        result_size,
        &mut result as *mut _ as *mut c_void,
        0 as *mut size_t,
    ) };

    eval_errcode(errcode, result, "clGetCommandQueueInfo",
        "functions::get_command_queue_context_ptr")
}


//============================================================================
//============================================================================
//====================== Wow, you made it this far? ==========================
//============================================================================
//============================================================================
