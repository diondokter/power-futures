#![feature(type_alias_impl_trait)]

use core::future::Future;
use std::{pin::Pin, task::Poll, time::Duration};
use tokio::sync::Mutex;

#[tokio::main]
async fn main() {
    static DOMAIN_MANAGER: DomainManager = DomainManager::new();

    let power_task = tokio::task::spawn(power_bahaviour(&DOMAIN_MANAGER));
    let task_0 = tokio::task::spawn(behaviour_block_0(&DOMAIN_MANAGER));
    let task_1 = tokio::task::spawn(behaviour_block_1(&DOMAIN_MANAGER));

    task_0.await.unwrap();
    task_1.await.unwrap();
    power_task.abort();
}

async fn power_bahaviour(domain_manager: &DomainManager) {
    let mut interval = tokio::time::interval(Duration::from_millis(10));

    loop {
        println!(
            "Required state: {:?}",
            domain_manager.determine_required_state().await
        );
        interval.tick().await;
    }
}

async fn behaviour_block_0(domain_manager: &'static DomainManager) {
    let id = domain_manager.register().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    println!("Power, from 0!");
    tokio::time::sleep(Duration::from_millis(100))
        .with_power(domain_manager, id, DomainState::High)
        .await;
    println!("Off, from 0!");
}

async fn behaviour_block_1(domain_manager: &'static DomainManager) {
    let id = domain_manager.register().await;
    println!("Power, from 1!");
    tokio::time::sleep(Duration::from_millis(200))
        .with_power(domain_manager, id, DomainState::Low)
        .await;
    println!("Off, from 1!");

    tokio::time::sleep(Duration::from_millis(100)).await;
}

pub struct DomainManager {
    requested_states: Mutex<Vec<DomainState>>,
}

impl DomainManager {
    pub const fn new() -> Self {
        Self {
            requested_states: Mutex::const_new(vec![]),
        }
    }

    pub async fn register(&self) -> DomainStateId {
        let mut requested_states = self.requested_states.lock().await;
        let old_length = requested_states.len();
        requested_states.push(DomainState::Off);
        DomainStateId(old_length)
    }

    pub async fn request_domain_state(
        &self,
        id: DomainStateId,
        requested_state: DomainState,
    ) -> DomainState {
        core::mem::replace(
            &mut self.requested_states.lock().await[id.0],
            requested_state,
        )
    }

    pub async fn determine_required_state(&self) -> DomainState {
        self.requested_states
            .lock()
            .await
            .iter()
            .fold(DomainState::Off, |l, r| l.combine(r))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum DomainState {
    High,
    Low,
    Off,
}

impl DomainState {
    #[must_use]
    pub fn combine(&self, other: &Self) -> Self {
        match (self, other) {
            (DomainState::High, _) => DomainState::High,
            (_, DomainState::High) => DomainState::High,
            (DomainState::Low, _) => DomainState::Low,
            (_, DomainState::Low) => DomainState::Low,
            (DomainState::Off, DomainState::Off) => DomainState::Off,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DomainStateId(usize);

pub struct PoweredFuture<
    S: Future<Output = DomainState>,
    F: Future,
    E: Future<Output = DomainState>,
    EG: Fn(DomainState) -> E,
> {
    start: S,
    inner: F,
    end_generator: EG,
    end: Option<E>,
    return_value: Option<F::Output>,
    state: u8,
}

impl<
        S: Future<Output = DomainState>,
        F: Future,
        E: Future<Output = DomainState>,
        EG: Fn(DomainState) -> E,
    > Future for PoweredFuture<S, F, E, EG>
{
    type Output = F::Output;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };

        match this.state {
            0 => {
                if let Poll::Ready(start_state) =
                    unsafe { Pin::new_unchecked(&mut this.start) }.poll(cx)
                {
                    this.state += 1;
                    this.end = Some((this.end_generator)(start_state));
                    cx.waker().wake_by_ref();
                }
                std::task::Poll::Pending
            }
            1 => {
                if let std::task::Poll::Ready(result) =
                    unsafe { Pin::new_unchecked(&mut this.inner) }.poll(cx)
                {
                    this.return_value = Some(result);
                    this.state += 1;
                    cx.waker().wake_by_ref();
                }
                std::task::Poll::Pending
            }
            2 => {
                if unsafe { Pin::new_unchecked(this.end.as_mut().unwrap()) }
                    .poll(cx)
                    .is_ready()
                {
                    this.state += 1;
                }
                std::task::Poll::Ready(this.return_value.take().unwrap())
            }
            _ => unreachable!(),
        }
    }
}

pub trait PowerExt<F: Future> {
    type S: Future<Output = DomainState>;
    type E: Future<Output = DomainState>;
    type EG: Fn(DomainState) -> Self::E;

    fn with_power(
        self,
        domain_manager: &'static DomainManager,
        id: DomainStateId,
        state: DomainState,
    ) -> PoweredFuture<Self::S, F, Self::E, Self::EG>;
}

impl<F: Future> PowerExt<F> for F {
    type S = impl Future<Output = DomainState>;
    type E = impl Future<Output = DomainState>;
    type EG = impl Fn(DomainState) -> Self::E;

    fn with_power(
        self,
        domain_manager: &'static DomainManager,
        id: DomainStateId,
        state: DomainState,
    ) -> PoweredFuture<Self::S, F, Self::E, Self::EG> {
        PoweredFuture {
            start: DomainManager::request_domain_state(domain_manager, id, state),
            inner: self,
            end: None,
            return_value: None,
            state: 0,
            end_generator: move |domain_state| {
                domain_manager.request_domain_state(id, domain_state)
            },
        }
    }
}
