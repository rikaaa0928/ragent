use openresponses_rust::Item;
use ragent::{
    AgentBuilder, AgentConfig, AgentContext, AgentEvent, ExtensionManager, FnEventHandler,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[tokio::test]
async fn builder_and_context_do_not_duplicate_system_prompt() {
    let config = AgentConfig::new("https://example.com", "fake_key", "test-model")
        .with_max_iterations(10)
        .with_temperature(0.5);
    let (agent, _) = AgentBuilder::new(config)
        .with_extension_manager(ExtensionManager::empty())
        .build()
        .await
        .unwrap();

    assert_eq!(
        agent.context().system_prompt(),
        Some("你是一个高效、精准、善于深度思考的 AI 智能体助手")
    );
    assert!(agent.context().items().is_empty());
}

#[tokio::test]
async fn sender_cancels_agent_before_model_io() {
    let config = AgentConfig::new("https://example.invalid", "fake_key", "test-model");
    let (mut agent, sender) = AgentBuilder::new(config)
        .with_extension_manager(ExtensionManager::empty())
        .build()
        .await
        .unwrap();
    let finished = Arc::new(AtomicUsize::new(0));
    let finished_for_handler = Arc::clone(&finished);
    agent.set_event_handler(Arc::new(FnEventHandler(move |event| {
        if matches!(event, AgentEvent::AgentFinished { .. }) {
            finished_for_handler.fetch_add(1, Ordering::SeqCst);
        }
    })));
    agent.add_user_message("this must not reach the model");

    sender.cancel();

    assert!(sender.is_cancelled());
    assert!(sender.cancellation_token().is_cancelled());
    assert_eq!(agent.run().await.unwrap(), "");
    assert_eq!(finished.load(Ordering::SeqCst), 1);
    assert!(agent.context().items().is_empty());
}

#[test]
fn context_keeps_complete_history() {
    let mut context = AgentContext::from_existing(vec![Item::system_message("old system")], None);
    for index in 1..=6 {
        context.add_user_message(format!("message {index}"));
    }
    assert_eq!(context.items().len(), 6);
    assert_eq!(context.system_prompt(), Some("old system"));
}
