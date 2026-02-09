#!/usr/bin/env bash
set -euo pipefail

REPO="lane2077/clash-cli.rs"
VERSION="latest"
MIRROR="auto"
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
  --repo OWNER/REPO        GitHub 仓库，默认 lane2077/clash-cli.rs
  --version TAG            CLI 版本，默认 latest（示例: v0.1.0）
  --mirror MODE            下载镜像: auto|ghfast|github，默认 auto
  --bin-path PATH          clash 安装路径，默认 /usr/local/bin/clash
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
    --repo)
      REPO="${2:-}"
      shift 2
      ;;
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --mirror)
      MIRROR="${2:-}"
      shift 2
      ;;
    --bin-path)
      CLASH_BIN_PATH="${2:-}"
      shift 2
      ;;
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

if ! command -v curl >/dev/null 2>&1; then
  echo "未检测到 curl，请先安装 curl。" >&2
  exit 1
fi

if ! command -v tar >/dev/null 2>&1; then
  echo "未检测到 tar，请先安装 tar。" >&2
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

case "$(uname -m)" in
  x86_64|amd64)
    ASSET="clash-linux-amd64.tar.gz"
    ;;
  *)
    echo "当前仅提供 Linux amd64 发布包，当前架构: $(uname -m)" >&2
    exit 1
    ;;
esac

if [[ "${VERSION}" != "latest" && "${VERSION}" != v* ]]; then
  VERSION="v${VERSION}"
fi

build_release_path() {
  if [[ "${VERSION}" == "latest" ]]; then
    echo "https://github.com/${REPO}/releases/latest/download/${ASSET}"
  else
    echo "https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"
  fi
}

add_mirror_prefix() {
  local url="$1"
  local mode="$2"
  case "${mode}" in
    ghfast)
      echo "https://ghfast.top/${url}"
      ;;
    github)
      echo "${url}"
      ;;
    *)
      echo "${url}"
      ;;
  esac
}

RELEASE_URL="$(build_release_path)"
DOWNLOAD_CANDIDATES=()
case "${MIRROR}" in
  auto)
    DOWNLOAD_CANDIDATES+=("$(add_mirror_prefix "${RELEASE_URL}" ghfast)")
    DOWNLOAD_CANDIDATES+=("$(add_mirror_prefix "${RELEASE_URL}" github)")
    ;;
  ghfast|github)
    DOWNLOAD_CANDIDATES+=("$(add_mirror_prefix "${RELEASE_URL}" "${MIRROR}")")
    ;;
  *)
    echo "无效 --mirror: ${MIRROR}（可选: auto|ghfast|github）" >&2
    exit 1
    ;;
esac

TMP_DIR="$(mktemp -d)"
ARCHIVE_PATH="${TMP_DIR}/${ASSET}"
cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

DOWNLOADED_URL=""
for u in "${DOWNLOAD_CANDIDATES[@]}"; do
  echo "尝试下载: ${u}"
  if curl -fL --connect-timeout 15 --retry 2 --retry-delay 1 -o "${ARCHIVE_PATH}" "${u}"; then
    DOWNLOADED_URL="${u}"
    break
  fi
done

if [[ -z "${DOWNLOADED_URL}" ]]; then
  echo "下载失败，请检查网络或仓库发布文件。" >&2
  exit 1
fi

tar -xzf "${ARCHIVE_PATH}" -C "${TMP_DIR}"
if [[ ! -f "${TMP_DIR}/clash-linux-amd64" ]]; then
  echo "发布包内容异常：未找到 clash-linux-amd64" >&2
  exit 1
fi

echo "安装 clash 到 ${CLASH_BIN_PATH} ..."
${SUDO} install -m 0755 "${TMP_DIR}/clash-linux-amd64" "${CLASH_BIN_PATH}"
echo "下载来源: ${DOWNLOADED_URL}"

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
  --mirror "${MIRROR}"
  --service-name "${SERVICE_NAME}"
  --workdir "${WORKDIR}"
)
if [[ "${NO_TUN}" -eq 1 ]]; then
  CMD+=(--no-tun)
fi

${SUDO} env CLASH_CLI_HOME="${CLASH_HOME}" "${CMD[@]}"

echo "安装与初始化完成。"
