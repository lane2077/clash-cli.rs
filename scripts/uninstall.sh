#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="clash-mihomo"
CLASH_HOME="/etc/clash-cli"
CLASH_BIN_PATH="/usr/local/bin/clash"
MIHOMO_BIN_PATH="/usr/local/bin/mihomo"
KEEP_HOME=0
KEEP_CORE=0
KEEP_CLI=0

usage() {
  cat <<'EOF'
用法:
  scripts/uninstall.sh [选项]

选项:
  --service-name NAME      systemd 服务名，默认 clash-mihomo
  --home PATH              CLASH_CLI_HOME，默认 /etc/clash-cli
  --keep-home              保留 CLASH_CLI_HOME 目录
  --keep-core              保留 /usr/local/bin/mihomo
  --keep-cli               保留 /usr/local/bin/clash
  -h, --help               显示帮助
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --service-name)
      SERVICE_NAME="${2:-}"
      shift 2
      ;;
    --home)
      CLASH_HOME="${2:-}"
      shift 2
      ;;
    --keep-home)
      KEEP_HOME=1
      shift
      ;;
    --keep-core)
      KEEP_CORE=1
      shift
      ;;
    --keep-cli)
      KEEP_CLI=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "未知参数: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "仅支持 Linux。" >&2
  exit 1
fi

if [[ "$(id -u)" -ne 0 ]]; then
  if ! command -v sudo >/dev/null 2>&1; then
    echo "请使用 root 执行，或安装 sudo。" >&2
    exit 1
  fi
  SUDO="sudo"
else
  SUDO=""
fi

run_best_effort() {
  set +e
  "$@"
  local code=$?
  set -e
  return ${code}
}

if [[ -x "${CLASH_BIN_PATH}" ]]; then
  echo "停止 tun 与 service..."
  run_best_effort ${SUDO} env CLASH_CLI_HOME="${CLASH_HOME}" "${CLASH_BIN_PATH}" tun off --name "${SERVICE_NAME}" >/dev/null
  run_best_effort ${SUDO} env CLASH_CLI_HOME="${CLASH_HOME}" "${CLASH_BIN_PATH}" service uninstall --name "${SERVICE_NAME}" --purge >/dev/null
else
  echo "未检测到 ${CLASH_BIN_PATH}，跳过 CLI 卸载流程。"
fi

echo "清理遗留数据面规则..."
run_best_effort ${SUDO} nft delete table inet clash_cli_tun >/dev/null
run_best_effort ${SUDO} iptables -t nat -F CLASH_CLI_TUN >/dev/null
run_best_effort ${SUDO} iptables -t nat -X CLASH_CLI_TUN >/dev/null
run_best_effort ${SUDO} ip6tables -t nat -F CLASH_CLI_TUN >/dev/null
run_best_effort ${SUDO} ip6tables -t nat -X CLASH_CLI_TUN >/dev/null

if [[ "${KEEP_HOME}" -eq 0 ]]; then
  echo "清理目录: ${CLASH_HOME}"
  run_best_effort ${SUDO} rm -rf "${CLASH_HOME}"
else
  echo "保留目录: ${CLASH_HOME}"
fi

if [[ "${KEEP_CORE}" -eq 0 ]]; then
  echo "删除内核: ${MIHOMO_BIN_PATH}"
  run_best_effort ${SUDO} rm -f "${MIHOMO_BIN_PATH}"
else
  echo "保留内核: ${MIHOMO_BIN_PATH}"
fi

if [[ "${KEEP_CLI}" -eq 0 ]]; then
  echo "删除 CLI: ${CLASH_BIN_PATH}"
  run_best_effort ${SUDO} rm -f "${CLASH_BIN_PATH}"
else
  echo "保留 CLI: ${CLASH_BIN_PATH}"
fi

echo "卸载完成。"
