//! scheduler

use async_std::channel;
use async_std::channel::unbounded;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type TimestampSeconds = u64;
fn timestamp_to_systime(s: TimestampSeconds) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(s)
}
pub struct EventList<T> {
    objects: Vec<Option<T>>,
    availables: usize,
}
pub struct MessageArena<T> {
    objects_times: BTreeMap<TimestampSeconds, EventList<T>>,
}
// スケジューラ
pub struct Scheduler<T, EH: EventHandler<T>> {
    message_arena: Arc<(Mutex<MessageArena<T>>, Condvar)>,
    _th: async_std::task::JoinHandle<()>,
    timeout_resetter: channel::Sender<()>,
    _ph: PhantomData<EH>,
}
pub struct CancellationToken(TimestampSeconds, usize);
impl<T: Send + 'static, EH: EventHandler<T> + Send + 'static> Scheduler<T, EH> {
    pub async fn new(handler: EH) -> Self {
        let message_arena = Arc::new((Mutex::new(MessageArena::new()), Condvar::new()));
        let (wait_send, waiter) = unbounded();
        let (timeout_resetter, timeout_signal) = unbounded();
        let message_arena_th = message_arena.clone();
        let _th = async_std::task::Builder::new()
            .name("Scheduler Thread".to_owned())
            .spawn(async move {
                wait_send.send(()).await.unwrap();
                loop {
                    let next_st = {
                        let mut lk = message_arena_th.0.lock().unwrap();
                        while lk.objects_times.is_empty() {
                            lk = message_arena_th.1.wait(lk).unwrap();
                        }
                        UNIX_EPOCH + Duration::from_secs(lk.next_estimated_time().unwrap())
                    };
                    if let Ok(d) = next_st.duration_since(SystemTime::now()) {
                        println!("scheduler reset: calling after {} secs", d.as_secs());
                        match async_std::future::timeout(d, timeout_signal.recv()).await {
                            Err(TimeoutError) => {
                                println!("Timeout!");
                                message_arena_th.0.lock().unwrap().process1(&handler);
                            }
                            Ok(Err(e)) => Err(e).unwrap(),
                            Ok(Ok(())) => println!("cancel!"),
                        }
                    } else {
                        println!("Timeout Early!");
                        let mut lk = message_arena_th.0.lock().unwrap();
                        while lk
                            .next_estimated_time()
                            .map(|ts| timestamp_to_systime(ts).duration_since(SystemTime::now()))
                            .map_or(false, |d| {
                                d.is_err() || d.unwrap() <= Duration::from_secs(0)
                            })
                        {
                            lk.process1(&handler);
                        }
                    }
                }
            })
            .unwrap();

        waiter.recv().await.unwrap();
        Scheduler {
            message_arena,
            timeout_resetter,
            _th,
            _ph: PhantomData,
        }
    }

    pub fn request(&self, t: TimestampSeconds, msg: T) -> CancellationToken {
        let mut l = self.message_arena.0.lock().unwrap();
        let id = l.add(t, msg);
        log::trace!("Registered as {}", id);

        CancellationToken(t, id)
    }
    pub fn cancel(&self, token: CancellationToken) {
        let mut l = self.message_arena.0.lock().unwrap();
        l.remove(token.0, token.1);
        log::trace!("Unregistered #{} from {}", token.1, token.0);
    }

    pub async fn reset_timeout(&self) {
        self.message_arena.1.notify_all();
        self.timeout_resetter.send(()).await.unwrap();
    }
}
impl<T> MessageArena<T> {
    pub fn new() -> Self {
        MessageArena {
            objects_times: BTreeMap::new(),
        }
    }
    pub fn next_estimated_time(&self) -> Option<TimestampSeconds> {
        self.objects_times.iter().next().map(|x| *x.0)
    }
    pub fn process1<H: EventHandler<T>>(&mut self, handler: &H) {
        if let Some(p) = self
            .next_estimated_time()
            .and_then(|x| self.objects_times.remove(&x))
        {
            handler.handle_batch(p.objects);
        }
    }
    pub fn add(&mut self, t: TimestampSeconds, object: T) -> usize {
        let sink = self.objects_times.entry(t).or_insert_with(|| EventList {
            objects: Vec::new(),
            availables: 0,
        });
        sink.objects.push(Some(object));
        sink.availables += 1;

        sink.availables - 1
    }
    pub fn remove(&mut self, t: TimestampSeconds, id: usize) {
        let remove = if let Some(to) = self.objects_times.get_mut(&t) {
            to.objects[id] = None;
            to.availables -= 1;
            to.availables <= 0
        } else {
            false
        };
        if remove {
            self.objects_times.remove(&t);
        }
    }
}

pub trait EventHandler<T> {
    fn handle_batch(&self, events: Vec<Option<T>>);
}
