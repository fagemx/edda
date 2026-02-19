use std::fs;
use std::path::{Path, PathBuf};

// ── Plugin Content ──

const PLUGIN_PACKAGE_JSON: &str = r#"{
  "name": "@edda/openclaw-bridge",
  "version": "0.1.0",
  "openclaw": {
    "extensions": [{ "entry": "index.js" }]
  }
}
"#;

const PLUGIN_INDEX_JS: &str = r#"const { execSync } = require("child_process");

const plugin = {
  id: "edda-bridge",
  name: "Edda Decision Memory",
  description: "Cross-session decision memory for coding agents",

  register(api) {
    const logger = api.logger;

    api.on("before_agent_start", async (event, ctx) => {
      const payload = JSON.stringify({
        hook_event_name: "before_agent_start",
        session_id: ctx.sessionId || "",
        session_key: ctx.sessionKey || "",
        agent_id: ctx.agentId || "main",
        workspace_dir: ctx.workspaceDir || "",
        event_data: { prompt: event.prompt },
      });

      try {
        const result = execSync("edda hook openclaw", {
          input: payload,
          encoding: "utf-8",
          timeout: 10000,
        });
        const parsed = JSON.parse(result);
        if (parsed.prependContext) {
          return { prependContext: parsed.prependContext };
        }
      } catch (err) {
        logger.warn("edda bridge: before_agent_start failed", err.message);
      }
    });

    api.on("agent_end", async (event, ctx) => {
      const payload = JSON.stringify({
        hook_event_name: "agent_end",
        session_id: ctx.sessionId || "",
        session_key: ctx.sessionKey || "",
        agent_id: ctx.agentId || "main",
        workspace_dir: ctx.workspaceDir || "",
        event_data: { success: event.success },
      });

      try {
        execSync("edda hook openclaw", {
          input: payload,
          encoding: "utf-8",
          timeout: 15000,
        });
      } catch (err) {
        logger.warn("edda bridge: agent_end failed", err.message);
      }
    });
  },
};

module.exports = plugin;
"#;

// ── Install ──

/// Default plugin directory.
fn default_plugin_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".openclaw").join("extensions").join("edda-bridge"))
}

/// Install the OpenClaw plugin files.
///
/// Creates `~/.openclaw/extensions/edda-bridge/` with `package.json` and `index.js`.
pub fn install(target_dir: Option<&Path>) -> anyhow::Result<()> {
    let dir = match target_dir {
        Some(d) => d.to_path_buf(),
        None => default_plugin_dir()
            .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?,
    };

    fs::create_dir_all(&dir)?;
    fs::write(dir.join("package.json"), PLUGIN_PACKAGE_JSON)?;
    fs::write(dir.join("index.js"), PLUGIN_INDEX_JS)?;

    println!("Installed edda OpenClaw plugin to {}", dir.display());
    println!();
    println!("To enable, add this to your OpenClaw config:");
    println!("  extensions: [\"{}\"]\n", dir.display());

    Ok(())
}

// ── Uninstall ──

/// Remove the OpenClaw plugin files.
pub fn uninstall(target_dir: Option<&Path>) -> anyhow::Result<()> {
    let dir = match target_dir {
        Some(d) => d.to_path_buf(),
        None => default_plugin_dir()
            .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?,
    };

    if dir.exists() {
        fs::remove_dir_all(&dir)?;
        println!("Removed edda OpenClaw plugin from {}", dir.display());
    } else {
        println!("No plugin found at {}", dir.display());
    }
    Ok(())
}

// ── Doctor ──

/// Check OpenClaw bridge health.
pub fn doctor() -> anyhow::Result<()> {
    // 1. Check edda in PATH
    let edda_in_path = which_edda();
    println!(
        "[{}] edda in PATH: {}",
        if edda_in_path.is_some() { "OK" } else { "WARN" },
        edda_in_path.unwrap_or_else(|| "not found".into())
    );

    // 2. Check plugin files exist
    let plugin_dir = default_plugin_dir();
    let has_plugin = plugin_dir
        .as_ref()
        .map(|d| d.join("index.js").exists())
        .unwrap_or(false);
    println!(
        "[{}] plugin installed: {}",
        if has_plugin { "OK" } else { "WARN" },
        plugin_dir
            .as_ref()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "unknown".into())
    );

    // 3. Check store root exists
    let root = edda_store::store_root();
    println!(
        "[{}] store root: {}",
        if root.exists() { "OK" } else { "WARN" },
        root.display()
    );

    Ok(())
}

fn which_edda() -> Option<String> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    let exe_name = if cfg!(windows) { "edda.exe" } else { "edda" };
    for dir in path_var.split(sep) {
        let candidate = Path::new(dir).join(exe_name);
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_creates_plugin_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("edda-bridge");
        install(Some(&dir)).unwrap();

        assert!(dir.join("package.json").exists());
        assert!(dir.join("index.js").exists());

        let pkg = fs::read_to_string(dir.join("package.json")).unwrap();
        assert!(pkg.contains("@edda/openclaw-bridge"));
        assert!(pkg.contains("openclaw"));

        let js = fs::read_to_string(dir.join("index.js")).unwrap();
        assert!(js.contains("edda hook openclaw"));
        assert!(js.contains("before_agent_start"));
        assert!(js.contains("agent_end"));
        assert!(js.contains("prependContext"));
    }

    #[test]
    fn uninstall_removes_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("edda-bridge");
        install(Some(&dir)).unwrap();
        assert!(dir.exists());

        uninstall(Some(&dir)).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn uninstall_nonexistent_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nonexistent");
        // Should not error
        uninstall(Some(&dir)).unwrap();
    }
}
