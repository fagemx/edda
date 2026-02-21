const { execSync } = require("child_process");

function callEdda(hookName, eventData, ctx, logger, timeout) {
  const payload = JSON.stringify({
    hook_event_name: hookName,
    session_id: ctx.sessionId || "",
    session_key: ctx.sessionKey || "",
    agent_id: ctx.agentId || "main",
    workspace_dir: ctx.workspaceDir || "",
    event_data: eventData,
  });
  try {
    const result = execSync("edda hook openclaw", {
      input: payload,
      encoding: "utf-8",
      timeout: timeout || 10000,
    });
    return JSON.parse(result);
  } catch (err) {
    logger.warn("edda bridge: " + hookName + " failed", err.message);
    return null;
  }
}

const plugin = {
  id: "edda-bridge",
  name: "Edda Decision Memory",
  description: "Cross-session decision memory for coding agents",

  register(api) {
    const logger = api.logger;

    api.on("session_start", async (event, ctx) => {
      callEdda("session_start", {}, ctx, logger, 15000);
    });

    api.on("before_agent_start", async (event, ctx) => {
      const result = callEdda(
        "before_agent_start",
        { prompt: event.prompt },
        ctx,
        logger,
        10000,
      );
      if (result && result.prependContext) {
        return { prependContext: result.prependContext };
      }
    });

    api.on("after_tool_call", async (event, ctx) => {
      const result = callEdda(
        "after_tool_call",
        {
          tool_name: event.toolName || "",
          tool_input: event.toolInput || {},
        },
        ctx,
        logger,
        5000,
      );
      if (result && result.additionalContext) {
        return { additionalContext: result.additionalContext };
      }
    });

    api.on("before_compaction", async (event, ctx) => {
      callEdda(
        "before_compaction",
        { session_file: ctx.sessionFile || "" },
        ctx,
        logger,
        5000,
      );
    });

    api.on("message_sent", async (event, ctx) => {
      callEdda(
        "message_sent",
        { text: event.text || "" },
        ctx,
        logger,
        5000,
      );
    });

    api.on("agent_end", async (event, ctx) => {
      callEdda(
        "agent_end",
        { success: event.success },
        ctx,
        logger,
        15000,
      );
    });

    api.on("session_end", async (event, ctx) => {
      callEdda(
        "session_end",
        { success: event.success },
        ctx,
        logger,
        15000,
      );
    });
  },
};

module.exports = plugin;
