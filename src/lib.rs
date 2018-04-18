
extern crate futures;

extern crate tokio;
extern crate tokio_timer;

extern crate slab;

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use futures::sync::mpsc::{self, UnboundedSender, UnboundedReceiver, SendError};
use futures::{task, Select, Future, Stream, Poll, Async};

use tokio_timer::{Timer, Sleep};

use std::io;
use std::time::{Instant, Duration};
use std::sync::{Arc, Mutex};

use std::ops::{Add, Sub};

use std::marker::PhantomData;

#[derive(Debug, Clone)]
pub struct Job<T> {
    inner: T,
    interval: Duration
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
struct PeriodicJob {
    key: usize,
    instant: Instant
}

impl Ord for PeriodicJob {
    fn cmp(&self, other: &PeriodicJob) -> Ordering {
        other.instant.cmp(&self.instant)
    }
}

impl PartialOrd for PeriodicJob {
    fn partial_cmp(&self, other: &PeriodicJob) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub struct Handler<T> {
    tx: UnboundedSender<Job<T>>
}

impl<T> Handler<T> {
    pub fn register(&self, interval: Duration, inner: T) -> Result<(), SendError<Job<T>>> {
        self.tx.unbounded_send(Job {
            inner,
            interval
        })
    }
}

#[derive(Debug)]
enum State<T> {
    Waiting(Receiver<T>),
    Timing(Select<Receiver<T>, Sleeper<T>>)
}

#[derive(Debug)]
pub struct PeriodicDispatcher<T> {
    timer: Timer,

    state: State<T>,
    slab: slab::Slab<Job<T>>,
    heap: BinaryHeap<PeriodicJob>,

    rx: Arc<Mutex<UnboundedReceiver<Job<T>>>>,
    tx: UnboundedSender<Job<T>>
}

impl<T> PeriodicDispatcher<T> {
    pub fn new(timer: Timer) -> PeriodicDispatcher<T> {
        let (tx, rx) = mpsc::unbounded();
        let rx = Arc::new(Mutex::new(rx));
        
        let initial_state = State::Waiting(Receiver::new(rx.clone()));

        PeriodicDispatcher {
            timer: timer,
            state: initial_state,
            slab: slab::Slab::new(),
            heap: BinaryHeap::new(),
            tx,
            rx,
        }
    }

    pub fn handle(&mut self) -> Handler<T> {
        Handler {
            tx: self.tx.clone()
        }
    }

    fn next_sleep_time(&self) -> Option<Duration> {
        let now = Instant::now();

        match self.heap.peek() {
            Some(job) => {
                let next = job.instant;

                if now.gt(&next) {
                    Some(Duration::from_secs(0))
                } else {
                    Some(next.sub(now))
                }
            },
            None => None
        }
    }
    
    fn add_job(&mut self, job: Job<T>) {
        let interval = job.interval;
        let key = self.slab.insert(job);
        self.dispatch_job(key, interval);
    }

    fn dispatch_job(&mut self, key: usize, interval: Duration) {
        let now = Instant::now();

        self.heap.push(PeriodicJob {
            key: key,
            instant: now.add(interval)
        });
    }

    fn set_state_waiting(&mut self, duration: Duration) -> State<T> {
        let sleeper     = Sleeper::new(self.timer.sleep(duration));
        let receiver    = Receiver::new(self.rx.clone());

        State::Timing(receiver.select(sleeper))
    }

    fn set_state_timing(&mut self) -> State<T> {
        State::Waiting(Receiver::new(self.rx.clone()))
    }
}

impl<T> Stream for PeriodicDispatcher<T> 
    where T: std::fmt::Debug + Clone
{
    type Item = T;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {

        let status = match self.state {
            State::Waiting(ref mut r) => {
                r.poll()
            },
            State::Timing(ref mut s) => {
                match s.poll() {
                    Ok(Async::Ready((msg, _)))  => Ok(Async::Ready(msg)),
                    Ok(Async::NotReady)         => Ok(Async::NotReady),
                    Err((err, _))               => Err(err),
                }
            }
        };
        
        match status {
            Err(err) => Err(err),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(msg)) => {
                let next_msg = match msg {
                    Action::Timeout => {
                        let (key, job) = {
                            let job_status = {
                                match self.heap.pop() {
                                    Some(job_status) => job_status,
                                    None => panic!("Should be handled")
                                }
                            };

                            let job = &self.slab[job_status.key];
                            let job = job.clone();

                            (job_status.key, job)
                        };

                        self.dispatch_job(key, job.interval);
                        Async::Ready(Some(job.inner))
                    },
                    Action::Update(job) => {
                        self.add_job(job);
                        Async::NotReady
                    }
                };
                
                self.state = {
                    match self.next_sleep_time() {
                        Some(duration) => self.set_state_waiting(duration),
                        None => self.set_state_timing(),
                    }
                };

                if let Async::NotReady = next_msg {
                    let task = task::current();
                    task.notify();
                };

                Ok(next_msg)
            }
        }
    }
}

#[derive(Debug)]
enum Action<T> {
    Update(Job<T>),
    Timeout
}

// --- timer

#[derive(Debug)]
struct Sleeper<T> {
    inner: Sleep,
    dummy: PhantomData<T>
}

impl<T> Sleeper<T> {
    fn new(inner: Sleep) -> Sleeper<T> {
        Sleeper {
            inner,
            dummy: PhantomData
        }
    }
}

impl<T> Future for Sleeper<T> {
    type Item = Action<T>;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.inner.poll()? {
            Async::NotReady => Ok(Async::NotReady),
            Async::Ready(()) => Ok(Async::Ready(Action::Timeout))
        }
    }
}

// --- receiver

#[derive(Debug)]
struct Receiver<T> {
    inner: Arc<Mutex<UnboundedReceiver<Job<T>>>>,
}

impl<T> Receiver<T> {
    fn new(inner: Arc<Mutex<UnboundedReceiver<Job<T>>>>) -> Receiver<T> {
        Receiver {
            inner
        }
    }
}

impl<T> Future for Receiver<T> {
    type Item = Action<T>;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let mut inner = self.inner.lock().unwrap();

        match inner.poll() {
            Err(_)                      => panic!("Error at receiver"),
            Ok(Async::NotReady)         => Ok(Async::NotReady),
            Ok(Async::Ready(None))      => Ok(Async::NotReady),
            Ok(Async::Ready(Some(job))) => Ok(Async::Ready(Action::Update(job)))
        }
    }
}
