mod cli_app;

use paradown::Error;

#[tokio::main]
async fn main() -> Result<(), Error> {
    cli_app::run().await
}
