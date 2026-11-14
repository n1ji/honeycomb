// Self-test plugin (see smoke.rs): keeps a secret that probe-b then tries to get at.
export async function activate(ctx) {
  await ctx.secrets.set("token", "probe-a-secret");
  await ctx.settings.set("result", (await ctx.secrets.get("token")) === "probe-a-secret" ? "ok" : "can't read its own secret");
}
