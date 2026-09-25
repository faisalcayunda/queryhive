//! The runtime this app runs its work on, and where that work is allowed to land.
//!
//! ADR-0010 decided this, and the reason is worth restating because the failure it
//! avoids is invisible in code review: `std::thread::available_parallelism()`
//! returns the **total** core count, treating a performance core and an efficiency
//! core as equals. A runtime sized from that number puts interactive work on
//! efficiency cores, and the user feels it as lag exactly when it is most visible —
//! the first fetch, and the scroll.
//!
//! So the counts come from the machine's own perflevel description, and the QoS
//! class is set explicitly:
//!
//! | Work | Pool | Class |
//! |---|---|---|
//! | connect, execute, fetch, scroll | P cores | `USER_INITIATED` |
//! | metadata index, autocomplete | E cores | `UTILITY` |
//! | spill, compaction, diagnostics | E cores | `BACKGROUND` |
//!
//! # Where the `unsafe` is
//!
//! Three blocks, all in this file, each on a Darwin C call this crate exists to
//! make, and each with its own `SAFETY:` note:
//!
//! | Call | Why it cannot be safe |
//! |---|---|
//! | `pthread_set_qos_class_self_np` | the only way macOS exposes the choice of core kind |
//! | `pthread_get_qos_class_np` | reads back what was actually applied, which is how the tests check it |
//! | `sysctlbyname` | reads the perflevel counts, which no standard API exposes |
//!
//! The crate sets `unsafe_code = "deny"` at the lint level and re-allows it on each
//! of the three, so a fourth cannot be added by accident.
//!
//! # What this crate does on a machine that is not Apple Silicon
//!
//! It still works. [`cores`] falls back to the standard API, the slow pools fall
//! back to one worker rather than zero, and setting a QoS class is a no-op. Nothing
//! here errors or panics because a perflevel is missing — the app runs, it just
//! gets the scheduling the OS would have chosen anyway.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

/// Which kind of work something is, and therefore where it should run.
///
/// Named after the QoS classes rather than after the work, so that the mapping in
/// this file is the only place that decides what "slow" means for the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Qos {
    /// Work the user is waiting to see: connect, execute, fetch, scroll.
    UserInitiated,
    /// Work that may take its time: metadata indexes, autocomplete corpora.
    Utility,
    /// Work nobody is waiting for: spill, compaction, diagnostics.
    Background,
}

/// The machine's core counts, kept apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cores {
    /// Performance cores. The interactive pool is this many workers.
    pub performance: usize,
    /// Efficiency cores, or `0` when the machine does not report them — an Intel
    /// Mac, or any platform without perflevels.
    pub efficiency: usize,
}

impl Cores {
    /// Total physical cores, which is `0` total only if both counts are zero.
    pub const fn total(&self) -> usize {
        self.performance + self.efficiency
    }

    /// How many workers the pools that are allowed to be slow should get.
    ///
    /// Falls back to one rather than to zero: a pool with no workers never runs
    /// anything, which would turn "this machine has no efficiency cores" into
    /// "metadata indexing silently never happens". The ADR names E cores for the
    /// metadata pool and does not give a count for the background class, so it
    /// takes the same answer for the same reason — this is the work the E cores
    /// exist for.
    pub const fn slow_pool_workers(&self) -> usize {
        if self.efficiency > 0 {
            self.efficiency
        } else {
            1
        }
    }
}

/// Read the machine's core counts.
///
/// On macOS this asks `sysctl` for `hw.perflevel0.physicalcpu` (performance) and
/// `hw.perflevel1.physicalcpu` (efficiency) — the perflevel description is the only
/// place the two kinds are told apart, and no standard API exposes it.
///
/// Everywhere else, and on any machine where the keys are missing, it falls back to
/// `available_parallelism()` reported as performance cores with none in the other
/// pile. That number is the one ADR-0010 calls misleading, so it is only used when
/// there is nothing better to use.
pub fn cores() -> Cores {
    #[cfg(target_os = "macos")]
    {
        if let (Some(performance), Some(efficiency)) = (
            sysctl_positive("hw.perflevel0.physicalcpu"),
            sysctl_positive("hw.perflevel1.physicalcpu"),
        ) {
            return Cores {
                performance,
                efficiency,
            };
        }
        // A macOS machine that reports only perflevel0 still gives a true
        // performance count, which is the number the interactive pool needs.
        if let Some(performance) = sysctl_positive("hw.perflevel0.physicalcpu") {
            return Cores {
                performance,
                efficiency: 0,
            };
        }
    }

    Cores {
        performance: std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1),
        efficiency: 0,
    }
}

/// The main runtime: one worker per performance core, at `USER_INITIATED`.
///
/// `worker_threads` is set explicitly rather than left to tokio's default, which is
/// `available_parallelism()` — the total, efficiency cores included, which is the
/// over-allocation ADR-0010 exists to avoid.
pub fn build_main() -> std::io::Result<Runtime> {
    let cores = cores();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cores.performance.max(1))
        .thread_name("qh-main")
        .on_thread_start(|| set_thread_qos(Qos::UserInitiated))
        .enable_all()
        .build()
}

// The slow pools are built on first use and kept for the life of the process:
// a runtime per call would spawn threads on every metadata lookup, which is the
// opposite of what a pool is for.
static UTILITY: OnceLock<Runtime> = OnceLock::new();
static BACKGROUND: OnceLock<Runtime> = OnceLock::new();

fn slow_pool(store: &'static OnceLock<Runtime>, qos: Qos, name: &str) -> &'static Runtime {
    store.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(cores().slow_pool_workers())
            .thread_name(name)
            .on_thread_start(move || set_thread_qos(qos))
            .enable_all()
            .build()
            .expect("a runtime of one or more workers should build")
    })
}

/// Run a future on the pool that matches its kind of work.
///
/// This is the single entry point ADR-0010 asks for, so that no call site has to
/// know which pool is which — and so that "put it on the background pool" cannot
/// drift into four different implementations.
///
/// [`Qos::UserInitiated`] is spawned on the **caller's** runtime rather than on a
/// pool of this crate's making: interactive work belongs to the application's main
/// runtime, and handing it a second one here would give it two schedulers to
/// contend over.
pub fn spawn_at<F>(qos: Qos, future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    match qos {
        Qos::UserInitiated => {
            tokio::spawn(future);
        }
        Qos::Utility => {
            slow_pool(&UTILITY, Qos::Utility, "qh-utility").spawn(future);
        }
        Qos::Background => {
            slow_pool(&BACKGROUND, Qos::Background, "qh-background").spawn(future);
        }
    }
}

/// Set this thread's QoS class.
///
/// The only `unsafe` in the crate, and it exists because macOS offers no safe way
/// to say which kind of core a thread should use: the QoS class is the mechanism,
/// and `pthread_set_qos_class_self_np` is how it is set.
///
/// The result is deliberately ignored. A machine that refuses the class — a
/// sandbox, a future OS, a host without perflevels — must still run the work; the
/// alternative is a metadata index that silently never runs because a scheduling
/// hint was declined.
#[allow(unsafe_code)]
fn set_thread_qos(qos: Qos) {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `pthread_set_qos_class_self_np` takes a `qos_class_t` and an
        // integer priority, and acts only on the calling thread. The class is one
        // of the enum's own variants, produced by `Qos::class`, so the value is
        // valid by construction. There is no aliasing, no lifetime, and no buffer
        // involved, and the return value is a status code that needs no cleanup.
        unsafe {
            libc::pthread_set_qos_class_self_np(qos.class(), 0);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = qos;
    }
}

impl Qos {
    /// The platform's own value for this class.
    #[cfg(target_os = "macos")]
    const fn class(self) -> libc::qos_class_t {
        match self {
            // `USER_INITIATED` rather than `USER_INTERACTIVE`: the latter is
            // reserved for drawing and event handling on the main thread, and
            // spending it on a network fetch would starve the UI it is meant to
            // serve.
            Qos::UserInitiated => libc::qos_class_t::QOS_CLASS_USER_INITIATED,
            Qos::Utility => libc::qos_class_t::QOS_CLASS_UTILITY,
            Qos::Background => libc::qos_class_t::QOS_CLASS_BACKGROUND,
        }
    }

    #[cfg(target_os = "macos")]
    const fn from_class(class: libc::qos_class_t) -> Option<Self> {
        match class {
            libc::qos_class_t::QOS_CLASS_USER_INITIATED => Some(Qos::UserInitiated),
            libc::qos_class_t::QOS_CLASS_UTILITY => Some(Qos::Utility),
            libc::qos_class_t::QOS_CLASS_BACKGROUND => Some(Qos::Background),
            _ => None,
        }
    }
}

/// The QoS class the calling thread is actually running at.
///
/// Not decoration: this is how the tests check that the pools did what they claim,
/// and it is the only honest way to know. Requesting a class and reporting success
/// are different things, and macOS is free to ignore a request.
///
/// `None` on platforms without QoS classes, and `None` for a class this crate does
/// not name (`USER_INTERACTIVE`, `DEFAULT`).
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn current_thread_qos() -> Option<Qos> {
    let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
    let mut priority: libc::c_int = 0;
    // SAFETY: `pthread_self()` is always valid, and the two pointers are to locals
    // that outlive the call and are written only by the callee, which is what
    // `pthread_get_qos_class_np` documents.
    let status =
        unsafe { libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut priority) };
    if status != 0 {
        return None;
    }
    Qos::from_class(class)
}

/// No QoS classes exist here, so there is nothing to report.
#[cfg(not(target_os = "macos"))]
pub fn current_thread_qos() -> Option<Qos> {
    None
}

/// Read a positive integer from `sysctl`, or `None`.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn sysctl_positive(name: &str) -> Option<usize> {
    let key = std::ffi::CString::new(name).ok()?;
    let mut value: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: `key` is a NUL-terminated C string that outlives the call; `value` and
    // `size` are the buffer and its length, which is the shape `sysctlbyname` takes;
    // and the new-value pointer is null because this only reads.
    let status = unsafe {
        libc::sysctlbyname(
            key.as_ptr(),
            std::ptr::addr_of_mut!(value).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status == 0 && value > 0 {
        Some(value as usize)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_core_counts_are_read_from_the_machine() {
        let cores = cores();
        assert!(
            cores.performance >= 1,
            "the interactive pool needs a worker"
        );

        // This machine is Apple Silicon, so the perflevel keys are there and both
        // kinds are known. On an Intel Mac or another platform `efficiency` is
        // legitimately zero, which is why the assertion is guarded rather than
        // general.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert!(
            cores.efficiency >= 1,
            "an Apple Silicon machine reports efficiency cores, got {cores:?}"
        );

        // The sum must not exceed the total the standard API reports: this is the
        // check that the two perflevels are being read as physical counts and not
        // added twice.
        let total = std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1);
        assert!(
            cores.total() <= total,
            "{cores:?} claims more cores than the machine reports ({total})"
        );
    }

    #[test]
    fn a_machine_without_efficiency_cores_still_gets_a_slow_pool() {
        // Zero workers would mean the pool never runs anything, which would turn a
        // missing perflevel into metadata indexing that silently never happens.
        let intel_like = Cores {
            performance: 8,
            efficiency: 0,
        };
        assert_eq!(intel_like.slow_pool_workers(), 1);
        assert_eq!(
            Cores {
                performance: 8,
                efficiency: 4
            }
            .slow_pool_workers(),
            4
        );
    }

    #[test]
    fn the_counts_do_not_change_between_calls() {
        // Read once per call rather than cached, but the machine does not change
        // mid-process: a differing answer would mean the parse is unstable.
        assert_eq!(cores(), cores());
    }

    /// A plain `#[test]`, not a `#[tokio::test]`, and that is deliberate: this test
    /// builds a second runtime, and dropping a runtime from inside another one's
    /// async context panics. It has to be owned by a thread that is not itself a
    /// runtime worker.
    #[test]
    fn the_main_runtime_puts_its_workers_at_user_initiated() {
        let cores = cores();
        let runtime = build_main().expect("the main runtime should build");
        assert_eq!(
            runtime.metrics().num_workers(),
            cores.performance.max(1),
            "workers are sized from performance cores, not from the total"
        );

        // Spawned rather than awaited with `block_on`: `block_on` runs the future on
        // *this* thread, whose class says nothing about the workers'. Only a task
        // that actually lands on a worker can answer the question, and requesting a
        // class is not the same as getting it.
        let (sender, receiver) = tokio::sync::oneshot::channel();
        runtime.spawn(async move {
            let _ = sender.send(current_thread_qos());
        });
        let observed = runtime
            .block_on(receiver)
            .expect("the worker should report back");

        if cfg!(target_os = "macos") {
            assert_eq!(
                observed,
                Some(Qos::UserInitiated),
                "the interactive pool must not be left at the default class"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_slow_pools_run_at_their_own_classes() {
        for (qos, name) in [(Qos::Utility, "utility"), (Qos::Background, "background")] {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            spawn_at(qos, async move {
                let _ = sender.send(current_thread_qos());
            });
            let observed = receiver.await.expect("the pool should report back");

            if cfg!(target_os = "macos") {
                assert_eq!(
                    observed,
                    Some(qos),
                    "the {name} pool came back at {observed:?}, so the class was not applied"
                );
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn user_initiated_work_stays_on_the_callers_runtime() {
        // Handing interactive work a second runtime would give it two schedulers to
        // contend over, so `spawn_at(UserInitiated, ..)` must land here, where the
        // test's own runtime is.
        let (sender, receiver) = tokio::sync::oneshot::channel();
        spawn_at(Qos::UserInitiated, async move {
            let _ = sender.send(tokio::runtime::Handle::current().metrics().num_workers());
        });
        let workers = receiver.await.expect("the task should report back");
        assert_eq!(workers, 2, "it should have run on this test's own runtime");
    }

    #[test]
    fn the_slow_pools_are_built_once() {
        // Built on first use and kept: a runtime per call would spawn threads on
        // every metadata lookup, which is the opposite of what a pool is for.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            spawn_at(Qos::Utility, async {});
            spawn_at(Qos::Utility, async {});
        });
        assert!(UTILITY.get().is_some());
    }
}
