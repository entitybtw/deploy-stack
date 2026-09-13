#!/bin/sh
# deploy-stack — установка в одну команду.
#   ./install.sh
# Генерирует .env со случайным паролем (если ещё нет) и поднимает стек.
set -e

cd "$(dirname "$0")"

rand_pass() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 12
  else
    head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n'
  fi
}

HOST_IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
[ -z "$HOST_IP" ] && HOST_IP="127.0.0.1"

if [ ! -f .env ]; then
  cp .env.example .env
  PASS="$(rand_pass)"
  # подставляем сгенерированный пароль и IP хоста
  sed -i "s|^DEPLOY_ADMIN_PASS=.*|DEPLOY_ADMIN_PASS=${PASS}|" .env
  sed -i "s|^WEB_PORT=.*|WEB_PORT=3000|" .env
  if grep -q '^#DEPLOY_HOST=' .env; then
    sed -i "s|^#DEPLOY_HOST=.*|DEPLOY_HOST=${HOST_IP}|" .env
  else
    echo "DEPLOY_HOST=${HOST_IP}" >> .env
  fi
  echo "✓ создан .env (пароль сгенерирован)"
else
  echo "• .env уже существует — оставляю как есть"
fi

mkdir -p deploy-data www

# shellcheck disable=SC1091
. ./.env 2>/dev/null || true

echo "• собираю и запускаю стек..."
docker compose up -d --build

echo
echo "═══════════════════════════════════════════════"
echo "  deploy-stack запущен"
echo "  Панель:  http://${HOST_IP}:${WEB_PORT:-3000}"
echo "  Логин:   ${DEPLOY_ADMIN_USER:-admin}"
echo "  Пароль:  ${DEPLOY_ADMIN_PASS}"
echo "═══════════════════════════════════════════════"
