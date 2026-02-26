use std::path::Path;

use edda_serve::ServeConfig;

pub fn execute(repo_root: &Path, bind: &str, port: u16) -> anyhow::Result<()> {
    let config = ServeConfig {
        bind: bind.to_string(),
        port,
    };
    tokio::runtime::Runtime::new()?.block_on(edda_serve::serve(repo_root, config))
}
