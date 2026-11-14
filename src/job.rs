//! Windows Job objects (OQ-035, TM-fwd-13): a process assigned to one is
//! ended with it, and Windows closes the job, ending them all, when Tern is
//! gone, a crash included.

use std::os::windows::io::AsRawHandle;
use std::process::Child;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};

/// A job that kills its processes when its last handle closes.
pub struct Job(HANDLE);

// SAFETY: a job handle is a kernel handle, usable from any thread.
unsafe impl Send for Job {}
// SAFETY: as above; the calls on it are thread-safe.
unsafe impl Sync for Job {}

impl Job {
    /// An empty job, or `None` if Windows refuses one.
    pub fn new() -> Option<Self> {
        // SAFETY: plain calls on a handle this function owns.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return None;
            }
            let job = Self(handle);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            (set != 0).then_some(job)
        }
    }

    /// A job with `child` in it, or `None` if Windows refuses one.
    pub fn holding(child: &Child) -> Option<Self> {
        Self::new().filter(|job| job.assign(child))
    }

    /// Puts `child` in the job; false if Windows refuses.
    pub fn assign(&self, child: &Child) -> bool {
        // SAFETY: the job's handle is open until `self` drops, and
        // `child`'s while it is borrowed.
        unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle().cast()) != 0 }
    }

    /// Ends every process in the job.
    pub fn kill(&self) {
        // SAFETY: the handle is open until `self` drops.
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `new` and is closed once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
