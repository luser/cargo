use std::env;
use std::fmt;
use std::fs::File;
use std::mem;
use std::process;
use std::time::{self, Duration, Instant};
use std::iter::repeat;
use std::cell::RefCell;
use std::io::{stdout, StdoutLock, Write};
use std::sync::{Once, ONCE_INIT};

use lazy_static;
use libc::{self, SYS_gettid};
use serde_json::Value;

thread_local!(static PROFILE_STACK: RefCell<Vec<time::Instant>> = RefCell::new(Vec::new()));
thread_local!(static MESSAGES: RefCell<Vec<Message>> = RefCell::new(Vec::new()));
thread_local!(static THREAD_ID: u64 = unsafe { libc::syscall(SYS_gettid) as u64 });
static mut PROFILE_FILE: Option<File> = None;
lazy_static! {
    static ref PROFILE_START: Instant = Instant::now();
}

type Message = (usize, u64, String);

pub struct Profiler {
    desc: String,
    args: Value,
}

fn enabled_level() -> Option<usize> {
    env::var("CARGO_PROFILE").ok().and_then(|s| s.parse().ok())
}

pub fn start<T: fmt::Display>(desc: T) -> Profiler {
    if enabled_level().is_none() { return Profiler { desc: String::new(), args: Value::Null } }
    lazy_static::initialize(&PROFILE_START);

    PROFILE_STACK.with(|stack| stack.borrow_mut().push(time::Instant::now()));

    Profiler {
        desc: desc.to_string(),
        args: Value::Null,
    }
}

impl Profiler {
    pub fn args(&mut self, args: Value) {
        self.args = args;
    }
}

fn in_micros(d: Duration) -> u64 {
    1000000 * d.as_secs() + (d.subsec_nanos() / 1000) as u64
}

impl Drop for Profiler {
    fn drop(&mut self) {
        let enabled = match enabled_level() {
            Some(i) => i,
            None => return,
        };

        let (start, stack_len) = PROFILE_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            let start = stack.pop().unwrap();
            (start, stack.len())
        });
        let duration = start.elapsed();
        let duration_ms =
            duration.as_secs() * 1000 + u64::from(duration.subsec_nanos() / 1_000_000);

        static INIT_LOG: Once = ONCE_INIT;
        INIT_LOG.call_once(|| unsafe {
            PROFILE_FILE = env::var("CARGO_PROFILE_FILE").ok()
                .and_then(|e| File::create(e).ok())
                .map(|mut f| {
                    drop(writeln!(f, "["));
                    f
                })
        });
        unsafe {
            match PROFILE_FILE {
                Some(ref mut f) => {
                    let ts = start.duration_since(*PROFILE_START);
                    let json_msg = json!({
                        "name": &self.desc,
                        "ph": "X",
                        "ts": in_micros(ts),
                        "dur": in_micros(duration),
                        "pid": process::id(),
                        "tid": THREAD_ID.with(|id| *id),
                        "args": self.args,
                    }).to_string();
                    drop(writeln!(f, "{},", json_msg));
                }
                None => {},
            }
        }

        let msg = (
            stack_len,
            duration_ms,
            mem::replace(&mut self.desc, String::new()),
        );
        MESSAGES.with(|msgs| msgs.borrow_mut().push(msg));


        if stack_len == 0 {
            fn print(lvl: usize, msgs: &[Message], enabled: usize, stdout: &mut StdoutLock) {
                if lvl > enabled {
                    return;
                }
                let mut last = 0;
                for (i, &(l, time, ref msg)) in msgs.iter().enumerate() {
                    if l != lvl {
                        continue;
                    }
                    writeln!(
                        stdout,
                        "{} {:6}ms - {}",
                        repeat("    ").take(lvl + 1).collect::<String>(),
                        time,
                        msg
                    ).expect("printing profiling info to stdout");

                    print(lvl + 1, &msgs[last..i], enabled, stdout);
                    last = i;
                }
            }
            let stdout = stdout();
            MESSAGES.with(|msgs| {
                let mut msgs = msgs.borrow_mut();
                print(0, &msgs, enabled, &mut stdout.lock());
                msgs.clear();
            });
        }
    }
}
