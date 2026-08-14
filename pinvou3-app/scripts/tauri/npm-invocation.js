const NPM_CI_ARGS = ["ci", "--prefer-offline", "--no-audit", "--no-fund"];

function npmInstallInvocation({
  platform = process.platform,
  environment = process.env,
  nodeExecutable = process.execPath,
  npmArgs = NPM_CI_ARGS,
} = {}) {
  const args = [...npmArgs];
  if (args.some((argument) => !/^[A-Za-z0-9@._=:/-]+$/u.test(argument))) {
    throw new Error("npm 参数包含不受支持的字符");
  }
  if (platform !== "win32") return { command: "npm", args };
  const npmExecPath = String(environment.npm_execpath || "").trim();
  if (npmExecPath && !/\.(?:cmd|bat)$/iu.test(npmExecPath)) {
    return { command: nodeExecutable, args: [npmExecPath, ...args] };
  }
  const commandInterpreter = String(
    environment.ComSpec || environment.COMSPEC || "cmd.exe",
  ).trim();
  return {
    command: commandInterpreter || "cmd.exe",
    args: ["/d", "/s", "/c", `npm.cmd ${args.join(" ")}`],
  };
}

module.exports = { NPM_CI_ARGS, npmInstallInvocation };
