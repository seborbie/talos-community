const environment = {
  ...process.env,
  PUBLIC_API_URL: process.env.PUBLIC_API_URL || "http://127.0.0.1:3001",
  PUBLIC_RMM_API_URL: process.env.PUBLIC_RMM_API_URL || "http://127.0.0.1:3002",
};

for (const argv of [
  ["bun", "x", "--bun", "svelte-kit", "sync"],
  ["bun", "x", "--bun", "svelte-check", "--tsconfig", "./tsconfig.json"],
]) {
  const child = Bun.spawn(argv, {
    cwd: import.meta.dir,
    env: environment,
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  });
  const exitCode = await child.exited;
  if (exitCode !== 0) {
    process.exit(exitCode);
  }
}
