//! 后台作业的观测上下文传播。

use std::future::Future;
use tokio::task::JoinHandle;
use tracing::{Instrument as _, instrument::WithSubscriber as _};

/// 捕获作业的订阅器与跨度，而不捕获通道转发器的生命周期。
#[derive(Clone, Debug)]
pub struct Context {
    span: tracing::Span,
    dispatch: tracing::Dispatch,
}

impl Context {
    /// 在发起工作的一侧同时捕获当前跨度与订阅器。
    pub fn current() -> Self {
        Self {
            span: tracing::Span::current(),
            dispatch: tracing::dispatcher::get_default(Clone::clone),
        }
    }

    /// 组合指定操作跨度与当前订阅器，供排队作业延后恢复。
    pub fn with_span(span: tracing::Span) -> Self {
        Self {
            span,
            dispatch: tracing::dispatcher::get_default(Clone::clone),
        }
    }

    /// 在每次轮询异步任务时进入捕获的上下文，不跨 await 持有进入守卫。
    pub fn instrument<F: Future>(self, future: F) -> impl Future<Output = F::Output> {
        future.instrument(self.span).with_subscriber(self.dispatch)
    }

    /// 只在同步闭包执行期间恢复上下文；异步任务应使用 `instrument`。
    pub fn in_scope<T>(&self, work: impl FnOnce() -> T) -> T {
        tracing::dispatcher::with_default(&self.dispatch, || self.span.in_scope(work))
    }
}

/// 将拥有所有权的同步工作移到阻塞线程，返回句柄由调用方等待和回收。
///
/// 上下文在调用时捕获。取消等待此句柄不保证已启动的阻塞闭包停止执行。
pub fn spawn_blocking<F: FnOnce() -> T + Send + 'static, T: Send + 'static>(
    work: F,
) -> JoinHandle<T> {
    // 在调用方线程捕获；阻塞工作线程不会继承 tracing 作用域。
    let context = Context::current();
    tokio::task::spawn_blocking(move || context.in_scope(work))
}
