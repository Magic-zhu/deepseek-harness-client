//! Windows Job Object guard: the daemon tree is assigned to a
//! kill-on-close job, so dropping the guard — or this process exiting —
//! takes cmd.exe, node, and every descendant down together. This is the
//! Windows counterpart of a Unix process group.

use std::io;
use std::mem::zeroed;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
};

/// Owning handle to a kill-on-close job. Drop closes the job.
pub struct JobGuard(HANDLE);

// The raw HANDLE is !Send by construction; this guard holds the only
// reference to it, and the kernel permits CloseHandle from any thread.
unsafe impl Send for JobGuard {}
unsafe impl Sync for JobGuard {}

/// Assign `pid` to a fresh kill-on-close job.
pub fn assign_pid(pid: u32) -> io::Result<JobGuard> {
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null_mut());
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &mut info as *mut _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            let err = io::Error::last_os_error();
            CloseHandle(job);
            return Err(err);
        }
        let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            let err = io::Error::last_os_error();
            CloseHandle(job);
            return Err(err);
        }
        let ok = AssignProcessToJobObject(job, process);
        CloseHandle(process);
        if ok == 0 {
            let err = io::Error::last_os_error();
            CloseHandle(job);
            return Err(err);
        }
        Ok(JobGuard(job))
    }
}

/// Hard-terminate one pid; escalation after the grace period expires.
pub fn terminate_pid(pid: u32) {
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !process.is_null() {
            TerminateProcess(process, 1);
            CloseHandle(process);
        }
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
