//! Windows timer resolution control for sub-millisecond fire precision.
//!
//! On Windows the default system timer resolution is ~15.6 ms, which means
//! `tokio::time::sleep(Duration::from_millis(1))` can actually sleep for up to
//! 15 ms. This module provides a RAII guard that calls `timeBeginPeriod(1)` to
//! force 1 ms resolution during critical fire windows, and restores the default
//! via `timeEndPeriod(1)` on drop.
//!
//! On non-Windows platforms this is a no-op.

/// RAII guard that holds elevated timer resolution while alive.
///
/// # Usage
/// ```ignore
/// let _guard = TimerResolutionGuard::activate();
/// // ... fire-critical timing code ...
/// // guard dropped here, resolution restored
/// ```
pub struct TimerResolutionGuard {
    #[cfg(target_os = "windows")]
    active: bool,
}

impl TimerResolutionGuard {
    /// Activate 1 ms timer resolution (Windows) or return a no-op guard (other OS).
    pub fn activate() -> Self {
        #[cfg(target_os = "windows")]
        {
            // SAFETY: timeBeginPeriod is a standard Windows multimedia API,
            // safe to call from any thread. Returns TIMERR_NOERROR (0) on success.
            let result = unsafe { windows_timer::timeBeginPeriod(1) };
            let active = result == 0;
            if active {
                crate::rlog!("Windows timer resolution set to 1 ms");
            } else {
                crate::rlog!("WARN: timeBeginPeriod(1) failed (code {})", result);
            }
            Self { active }
        }
        #[cfg(not(target_os = "windows"))]
        {
            Self {}
        }
    }
}

impl Drop for TimerResolutionGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        {
            if self.active {
                unsafe {
                    windows_timer::timeEndPeriod(1);
                }
                crate::rlog!("Windows timer resolution restored to default");
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_timer {
    // Link against winmm.lib for multimedia timer functions.
    #[link(name = "winmm")]
    extern "system" {
        pub fn timeBeginPeriod(uPeriod: u32) -> u32;
        pub fn timeEndPeriod(uPeriod: u32) -> u32;
    }
}
