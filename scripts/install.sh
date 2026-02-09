#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

PROFILE_URL=""
PROFILE_NAME="main"
SERVICE_NAME="clash-mihomo"
CLASH_HOME="/etc/clash-cli"
WORKDIR="/var/lib/clash-cli"
CLASH_BIN_PATH="/usr/local/bin/clash"
SKIP_SETUP=0
NO_TUN=0

usage() {
  cat <<'EOF'
用法:
  scripts/install.sh [选项]

选项:
  --profile-url URL        订阅地址（提供后会自动执行 setup init）
  --profile-name NAME      profile 名称，默认 main
  --service-name NAME      systemd 服务名，默认 clash-mihomo
  --home PATH              CLASH_CLI_HOME，默认 /etc/clash-cli
  --workdir PATH           service 工作目录，默认 /var/lib/clash-cli
  --skip-setup             仅安装 clash 二进制，不执行 setup init
  --no-tun                 setup init 时不自动开启 tun
  -h, --help               显示帮助
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile-url)
      PROFILE_URL="${2:-}"
      shift 2
      ;;
    --profile-name)
      PROFILE_NAME="${2:-}"
      shift 2
      ;;
    --service-name)
      SERVICE_NAME="${2:-}"
      shift 2
      ;;
    --home)
      CLASH_HOME="${2:-}"
      shift 2
      ;;
    --workdir)
      WORKDIR="${2:-}"
      shift 2
      ;;
    --skip-setup)
      SKIP_SETUP=1
      shift
      ;;
    --no-tun)
      NO_TUN=1
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

echo "构建 release 二进制..."
cargo build --release --manifest-path "${REPO_ROOT}/Cargo.toml"

echo "安装 clash 到 ${CLASH_BIN_PATH} ..."
${SUDO} install -m 0755 "${REPO_ROOT}/target/release/clash" "${CLASH_BIN_PATH}"

if [[ "${SKIP_SETUP}" -eq 1 ]]; then
  echo "已完成二进制安装。你可稍后手动执行:"
  echo "  sudo env CLASH_CLI_HOME=${CLASH_HOME} ${CLASH_BIN_PATH} setup init --profile-url <URL>"
  exit 0
fi

if [[ -z "${PROFILE_URL}" ]]; then
  echo "未提供 --profile-url，已仅完成二进制安装。"
  echo "后续执行:"
  echo "  sudo env CLASH_CLI_HOME=${CLASH_HOME} ${CLASH_BIN_PATH} setup init --profile-url <URL>"
  exit 0
fi

echo "执行 setup init ..."
CMD=(
  "${CLASH_BIN_PATH}" setup init
  --profile-url "${PROFILE_URL}"
  --profile-name "${PROFILE_NAME}"
  --service-name "${SERVICE_NAME}"
  --workdir "${WORKDIR}"
)
if [[ "${NO_TUN}" -eq 1 ]]; then
  CMD+=(--no-tun)
fi

${SUDO} env CLASH_CLI_HOME="${CLASH_HOME}" "${CMD[@]}"

echo "安装与初始化完成。"
