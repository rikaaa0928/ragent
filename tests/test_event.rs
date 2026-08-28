use openresponses_rust::Item;
use ragent::{AgentEvent, EventHandler, FnEventHandler, JsonLinesEventHandler, TokenUsage};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[test]
fn event_handler_closure_and_jsonl_work() {
    let counter = Arc::new(AtomicUsize::new(0));
    let cloned = Arc::clone(&counter);
    FnEventHandler(move |_| {
        cloned.fetch_add(1, Ordering::SeqCst);
    })
    .on_event(&AgentEvent::AgentFinished { total_usage: None });
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    let bytes = Arc::new(Mutex::new(Vec::new()));
    struct Writer(Arc<Mutex<Vec<u8>>>);
    impl Write for Writer {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    JsonLinesEventHandler::new(Box::new(Writer(Arc::clone(&bytes)))).on_event(
        &AgentEvent::TurnCompleted {
            iteration: 1,
            text: "hi".into(),
            reasoning: None,
            usage: Some(TokenUsage::new(100, 50, 150, 20, 10)),
        },
    );
    let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    let parsed = serde_json::from_str::<serde_json::Value>(output.trim()).unwrap();
    assert_eq!(parsed["type"], "turn_completed");
    assert_eq!(parsed["usage"]["total_tokens"], 150);
    assert_eq!(parsed["usage"]["cached_tokens"], 20);
    assert_eq!(parsed["usage"]["reasoning_tokens"], 10);
}

#[test]
fn test_reasoning_deserialization() {
    let json_str = r#"{
        "type": "reasoning",
        "id": "rs_123",
        "summary": [
            {
                "type": "summary_text",
                "text": "Hello thinking"
            }
        ]
    }"#;
    let item: Item = serde_json::from_str(json_str).unwrap();
    println!("Deserialized Item: {:?}", item);
}

#[test]
fn token_usage_aggregation_and_formatting() {
    let mut total = TokenUsage::default();
    let u1 = TokenUsage::new(100, 50, 150, 30, 20);
    let u2 = TokenUsage::new(200, 80, 280, 50, 40);

    total += &u1;
    assert_eq!(total.input_tokens, 100);
    assert_eq!(total.output_tokens, 50);
    assert_eq!(total.total_tokens, 150);
    assert_eq!(total.cached_tokens, 30);
    assert_eq!(total.reasoning_tokens, 20);

    total += u2;
    assert_eq!(total.input_tokens, 300);
    assert_eq!(total.output_tokens, 130);
    assert_eq!(total.total_tokens, 430);
    assert_eq!(total.cached_tokens, 80);
    assert_eq!(total.reasoning_tokens, 60);

    assert_eq!(
        total.formatted_details(),
        "总计: 430 (输入: 300, 输出: 130, 缓存: 80, 思考: 60)"
    );
}
