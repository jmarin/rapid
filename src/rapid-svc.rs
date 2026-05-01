use rapid::magic::mime_type_magic;
use std::path::Path;

// Temporary main. This will run the Axum web service
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mime_type = mime_type_magic(Path::new("data/files/NGC 6888.jpg")).await?;
    println!("Detected MIME type: {}", mime_type);
    Ok(())
}
