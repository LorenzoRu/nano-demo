use nano_core::ChatMessage;
use std::io::Write;
use std::sync::atomic::AtomicBool;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let messages = vec![ChatMessage {
        role: "user".into(),
        content: "Dis bonjour en une phrase.".into(),
    }];
    let cancel = AtomicBool::new(false);
    nano_core::stream_chat(
        "qwen2.5-coder:3b",
        &messages,
        |t| {
            print!("{t}");
            std::io::stdout().flush().ok();
        },
        &cancel,
    )
    .await?;
    println!();
    Ok(())
}
