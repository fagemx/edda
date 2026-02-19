const { execSync } = require("child_process");

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
