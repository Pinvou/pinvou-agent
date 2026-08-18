#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
asr_root="${PINVOU3_QWEN3_ASR_ROOT:-${HOME}/.pinvou3/asr/qwen3-asr-openvino}"
model_dir="${PINVOU3_QWEN3_ASR_MODEL_DIR:-${asr_root}/model}"
uv_bin="${UV_BIN:-${HOME}/.local/bin/uv}"
unit_dir="${HOME}/.config/systemd/user"

if [[ ! -x "${uv_bin}" ]]; then
  echo "uv is required at ${uv_bin}; install or copy a verified uv binary first" >&2
  exit 1
fi

install -d -m 700 "${asr_root}" "${asr_root}/cache" "${asr_root}/samples" "${unit_dir}"
install -m 700 "${script_dir}/qwen3-asr-openvino" "${asr_root}/qwen3-asr-openvino"
install -m 600 "${script_dir}/qwen3-asr-openvino.py" "${asr_root}/qwen3-asr-openvino.py"
install -m 600 "${script_dir}/qwen3-asr-model.sha256" "${asr_root}/qwen3-asr-model.sha256"
install -m 600 "${script_dir}/pinvou-qwen3-asr.service" "${unit_dir}/pinvou-qwen3-asr.service"

(
  cd "${model_dir}"
  sha256sum --check "${asr_root}/qwen3-asr-model.sha256"
)

"${uv_bin}" venv --python /usr/bin/python3 --clear "${asr_root}/runtime"
"${uv_bin}" pip install --python "${asr_root}/runtime/bin/python" \
  "openvino==2026.3.0" \
  "openvino-tokenizers==2026.3.0.0" \
  "openvino-genai==2026.3.0.0"

"${asr_root}/qwen3-asr-openvino" check
systemctl --user daemon-reload
systemctl --user enable --now pinvou-qwen3-asr.service
