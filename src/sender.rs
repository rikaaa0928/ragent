use crate::error::AgentError;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

/// Agent 输入端发送句柄，支持及时和延时双通道消息发送
#[derive(Clone, Debug)]
pub struct AgentSender {
    pub(crate) immediate_tx: UnboundedSender<String>,
    pub(crate) delayed_tx: UnboundedSender<String>,
    cancellation: CancellationToken,
}

impl AgentSender {
    pub fn new(immediate_tx: UnboundedSender<String>, delayed_tx: UnboundedSender<String>) -> Self {
        Self::with_cancellation(immediate_tx, delayed_tx, CancellationToken::new())
    }

    pub(crate) fn with_cancellation(
        immediate_tx: UnboundedSender<String>,
        delayed_tx: UnboundedSender<String>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            immediate_tx,
            delayed_tx,
            cancellation,
        }
    }

    /// 发送及时消息：在每个推理/执行循环迭代中都会被立即检查并追加到模型上下文中
    pub fn send_immediate(&self, msg: impl Into<String>) -> Result<(), AgentError> {
        self.immediate_tx.send(msg.into()).map_err(|e| {
            AgentError::ChannelError(format!("Failed to send immediate message: {}", e))
        })
    }

    /// 发送延时消息：在当前轮次（包括多步 Tool Calls 执行完后）才会被拉取并继续下一轮对话
    pub fn send_delayed(&self, msg: impl Into<String>) -> Result<(), AgentError> {
        self.delayed_tx
            .send(msg.into())
            .map_err(|e| AgentError::ChannelError(format!("Failed to send delayed message: {}", e)))
    }

    /// 检查两个输入通道是否均已关闭
    pub fn is_closed(&self) -> bool {
        self.immediate_tx.is_closed() && self.delayed_tx.is_closed()
    }

    /// 请求 Agent 尽快停止当前推理、Hook 或工具调用。
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Agent 是否已收到取消请求。
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    /// 返回与 Agent 共享的取消令牌，供外部异步任务组合等待。
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}
