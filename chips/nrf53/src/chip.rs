use crate::deferred_call_tasks::DeferredCallTask;
use crate::interrupt_service::InterruptService;
use crate::nvmc;
use core::fmt::Write;
use cortexm33::{self, nvic};
use kernel::common::deferred_call;
use kernel::debug;

pub struct NRF53<I: InterruptService> {
    mpu: cortexm33::mpu::MPU,
    userspace_kernel_boundary: cortexm33::syscall::SysCall,
    scheduler_timer: cortexm33::systick::SysTick,
    interrupt_service: I,
}

impl<I: InterruptService> NRF53<I> {
    pub unsafe fn new() -> NRF53<I> {
        NRF53 {
            mpu: cortexm33::mpu::MPU::new(),
            userspace_kernel_boundary: cortexm33::syscall::SysCall::new(),
            // The NRF53's systick is uncalibrated, but is clocked from the
            // 64Mhz CPU clock.
            scheduler_timer: cortexm33::systick::SysTick::new_with_calibration(64000000),
            interrupt_service: I::new(),
        }
    }
}

impl<I: InterruptService> kernel::Chip for NRF53<I> {
    type MPU = cortexm33::mpu::MPU;
    type UserspaceKernelBoundary = cortexm33::syscall::SysCall;
    type SchedulerTimer = cortexm33::systick::SysTick;
    type WatchDog = ();

    fn mpu(&self) -> &Self::MPU {
        &self.mpu
    }

    fn scheduler_timer(&self) -> &Self::SchedulerTimer {
        &self.scheduler_timer
    }

    fn watchdog(&self) -> &Self::WatchDog {
        &()
    }

    fn userspace_kernel_boundary(&self) -> &Self::UserspaceKernelBoundary {
        &self.userspace_kernel_boundary
    }

    fn service_pending_interrupts(&self) {
        unsafe {
            loop {
                if let Some(task) = deferred_call::DeferredCall::next_pending() {
                    match task {
                        // TODO: don't do for network core
                        DeferredCallTask::Nvmc => nvmc::NVMC_APP.handle_interrupt(),
                    }
                } else if let Some(interrupt) = nvic::next_pending() {
                    if !self.interrupt_service.service_interrupt(interrupt) {
                        debug!("NvicIdx not supported by Tock: {}", interrupt);
                    }
                    let n = nvic::Nvic::new(interrupt);
                    n.clear_pending();
                    n.enable();
                } else {
                    break;
                }
            }
        }
    }

    fn has_pending_interrupts(&self) -> bool {
        unsafe { nvic::has_pending() || deferred_call::has_tasks() }
    }

    fn sleep(&self) {
        unsafe {
            cortexm33::support::wfi();
        }
    }

    unsafe fn atomic<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        cortexm33::support::atomic(f)
    }

    unsafe fn print_state(&self, write: &mut dyn Write) {
        cortexm33::print_cortexm33_state(write);
    }
}
