const WRAPPER_ENV = "PINVOU3_TAURI_WRAPPED";

function requireWrapper(environment = process.env) {
  if (environment[WRAPPER_ENV] !== "1") {
    throw new Error(
      "禁止绕过平台 overlay 直接执行 Tauri build/bundle；请使用 npm run build、npm run tauri -- build 或对应平台构建脚本。",
    );
  }
}

if (require.main === module) {
  try {
    requireWrapper();
  } catch (error) {
    console.error(`[tauri] ${error.message}`);
    process.exitCode = 1;
  }
}

module.exports = { requireWrapper, WRAPPER_ENV };
