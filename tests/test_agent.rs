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

#[tokio::test]
async fn context_summary_filtering_behavior() {
    use openresponses_rust::MessageContent;
    use ragent::ContextSummaryMode;

    let reasoning_item = Item::Reasoning {
        id: Some("rs_1".into()),
        status: None,
        content: None,
        summary: vec![MessageContent::SummaryText {
            text: "thought".into(),
        }],
        encrypted_content: None,
    };
    let user_item = Item::user_message("hello");

    // 1. ContextSummaryMode::On retains all
    let config_on = AgentConfig::new("https://example.com", "k", "m")
        .with_context_summary(ContextSummaryMode::On);
    let (mut agent_on, _) = AgentBuilder::new(config_on)
        .with_extension_manager(ExtensionManager::empty())
        .build()
        .await
        .unwrap();
    agent_on.context_mut().add_item(user_item.clone());
    agent_on.context_mut().add_item(reasoning_item.clone());

    let all_items = agent_on.context().to_items();
    let items_on: Vec<Item> = match agent_on.config().context_summary {
        ContextSummaryMode::On => all_items,
        _ => unreachable!(),
    };
    assert_eq!(items_on.len(), 2);

    // 2. ContextSummaryMode::Off strips reasoning
    let config_off = AgentConfig::new("https://example.com", "k", "m")
        .with_context_summary(ContextSummaryMode::Off);
    let (mut agent_off, _) = AgentBuilder::new(config_off)
        .with_extension_manager(ExtensionManager::empty())
        .build()
        .await
        .unwrap();
    agent_off.context_mut().add_item(user_item.clone());
    agent_off.context_mut().add_item(reasoning_item.clone());

    let all_items = agent_off.context().to_items();
    let items_off: Vec<Item> = match agent_off.config().context_summary {
        ContextSummaryMode::Off => all_items
            .into_iter()
            .filter(|item| !matches!(item, Item::Reasoning { .. }))
            .collect(),
        _ => unreachable!(),
    };
    assert_eq!(items_off.len(), 1);

    // 3. ContextSummaryMode::Auto: initial session items kept, new items filtered
    let session = ragent::SessionData {
        meta: ragent::SessionMeta {
            id: "sess_1".into(),
            title: "t".into(),
            model: "m".into(),
            created_at: 0,
            updated_at: 0,
            item_count: 2,
        },
        system_prompt: None,
        items: vec![user_item.clone(), reasoning_item.clone()],
    };
    let config_auto = AgentConfig::new("https://example.com", "k", "m")
        .with_context_summary(ContextSummaryMode::Auto);
    let (mut agent_auto, _) = AgentBuilder::from_session_with_manager(
        session,
        config_auto,
        Some(ExtensionManager::empty()),
    )
    .await
    .unwrap();
    assert_eq!(agent_auto.initial_items_count(), 2);

    // Add a newly generated reasoning item in current run
    let new_reasoning = Item::Reasoning {
        id: Some("rs_2".into()),
        status: None,
        content: None,
        summary: vec![MessageContent::SummaryText {
            text: "new thought".into(),
        }],
        encrypted_content: None,
    };
    agent_auto.context_mut().add_item(new_reasoning);

    let all_items = agent_auto.context().to_items();
    let initial_len = agent_auto.initial_items_count();
    let items_auto: Vec<Item> = match agent_auto.config().context_summary {
        ContextSummaryMode::Auto => all_items
            .into_iter()
            .enumerate()
            .filter(|(idx, item)| {
                if matches!(item, Item::Reasoning { .. }) {
                    *idx < initial_len
                } else {
                    true
                }
            })
            .map(|(_, item)| item)
            .collect(),
        _ => unreachable!(),
    };
    // initial reasoning item (index 1 < 2) is kept, new reasoning item (index 2 >= 2) is filtered!
    assert_eq!(items_auto.len(), 2);
    assert_eq!(items_auto[1], reasoning_item);
}
