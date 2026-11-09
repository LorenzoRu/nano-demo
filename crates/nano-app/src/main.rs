use std::io::Write;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    nano_core::stream_chat("qwen2.5-coder:3b", "Dis bonjour en une phrase.", |t| {
        print!("{t}");
        std::io::stdout().flush().ok();
    })
    .await?;
    println!();
    Ok(())
}