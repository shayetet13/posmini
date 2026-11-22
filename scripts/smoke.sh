#!/usr/bin/env bash
# Smoke test for a running posmini (the Railway frontend URL, or the local docker stack).
#
#   scripts/smoke.sh https://frontend-production-xxxx.up.railway.app
#
# Without credentials it only makes public / read-only requests, so it is safe to point at
# production. With SMOKE_ADMIN_PASSWORD it also logs in; add SMOKE_WRITE=1 to record one real
# test sale (do that only against a throw-away stack such as CI's).
set -u

BASE="${1:?usage: smoke.sh <base-url>}"
BASE="${BASE%/}"
USER_NAME="${SMOKE_ADMIN_USER:-admin}"
PASSWORD="${SMOKE_ADMIN_PASSWORD:-}"
JAR="$(mktemp)"
trap 'rm -f "$JAR"' EXIT

failures=0
pass() { echo "ok    $1"; }
fail() { echo "FAIL  $1${2:+ ($2)}"; failures=$((failures + 1)); }

status() { curl -s -o /dev/null -w '%{http_code}' --max-time 20 "$@"; }
expect_status() { # name want url [curl args...]
  local name=$1 want=$2 url=$3
  shift 3
  local got
  got=$(status "$@" "$BASE$url")
  if [ "$got" = "$want" ]; then pass "$name"; else fail "$name" "wanted $want, got $got"; fi
}
expect_body() { # name pattern url [curl args...]
  local name=$1 pattern=$2 url=$3
  shift 3
  if curl -s --max-time 20 "$@" "$BASE$url" | grep -q -- "$pattern"; then pass "$name"; else fail "$name" "no '$pattern' in $url"; fi
}

echo "smoke test against $BASE"

# ---- public site
expect_status "frontend health"            200 /healthz
expect_status "login page"                 200 /login
expect_body   "POS page loads its script"  '/static/pos.js' /pos
expect_status "stylesheet"                 200 /static/app.css
expect_status "print station script"       200 /static/station.js
expect_body   "shop name from the backend" 'window.SHOP = ' /shop.js

redirect=$(curl -s -o /dev/null -w '%{redirect_url}' --max-time 20 "$BASE/")
case "$redirect" in */pos) pass "root redirects to /pos" ;; *) fail "root redirects to /pos" "got '$redirect'" ;; esac

# ---- the backend is reachable through the proxy, and protected
expect_status "API needs a login"          401 /api/orders/active
expect_status "print queue needs a login"  401 /api/print/queue
expect_status "station claim needs login"  401 /api/print/station/claim -X POST -H 'Content-Type: application/json' -d '{"station":"smoke"}'

bad=$(curl -s -o /dev/null -w '%{http_code} %{redirect_url}' --max-time 20 -X POST \
  --data-urlencode "username=$USER_NAME" --data-urlencode "password=smoke-wrong-password" "$BASE/login")
case "$bad" in "303 "*"/login?error=1") pass "wrong password is refused" ;; *) fail "wrong password is refused" "got '$bad'" ;; esac

nothing=$(printf 'a%.0s' $(seq 1 64))
expect_status "unknown order page"         200 "/q/$nothing"
expect_status "unknown order API"          404 "/api/q/$nothing"

slug=$(curl -s --max-time 20 "$BASE/shop.js" | sed -n 's/^window.SLUG = "\(.*\)";$/\1/p')
if [ -n "$slug" ]; then
  # Live updates must reach the browser through the proxy unbuffered.
  headers=$(curl -s -N --max-time 4 -D - -o /dev/null "$BASE/events/display/$slug" 2>/dev/null)
  if echo "$headers" | grep -qi 'content-type: text/event-stream'; then pass "live updates (SSE) stream"; else fail "live updates (SSE) stream" "no event-stream header"; fi
else
  fail "shop slug" "could not read it from /shop.js"
fi

# ---- signed-in checks (optional)
if [ -n "$PASSWORD" ]; then
  login=$(curl -s -o /dev/null -w '%{http_code}' --max-time 20 -c "$JAR" -X POST \
    --data-urlencode "username=$USER_NAME" --data-urlencode "password=$PASSWORD" "$BASE/login")
  if [ "$login" = "303" ] && grep -q sid "$JAR"; then pass "admin login"; else fail "admin login" "status $login"; fi

  expect_status "signed in: orders"        200 /api/orders/active -b "$JAR"
  expect_status "signed in: settings"      200 /api/settings -b "$JAR"
  expect_status "signed in: print queue"   200 /api/print/queue -b "$JAR"
  expect_status "signed in: dashboard"     200 "/api/dashboard?period=day&date=$(date -u +%Y-%m-%d)" -b "$JAR"

  if [ "${SMOKE_WRITE:-}" = "1" ]; then
    product=$(curl -s --max-time 20 -b "$JAR" "$BASE/api/products" | grep -o '"id":[0-9]*' | head -1 | cut -d: -f2)
    if [ -z "$product" ]; then
      fail "test sale" "no product to sell"
    else
      sale=$(curl -s --max-time 20 -b "$JAR" -X POST -H 'Content-Type: application/json' \
        -d "{\"method\":\"cash\",\"items\":[{\"product_id\":$product,\"qty\":1}],\"cash_received_satang\":100000}" "$BASE/api/orders")
      echo "$sale" | grep -q '"queue_label":"' && echo "$sale" | grep -q '"change_satang":' \
        && pass "cash sale gets a queue number and change" || fail "cash sale" "$sale"
      token=$(echo "$sale" | grep -o '"token":"[0-9a-f]\{64\}"' | head -1 | cut -d'"' -f4)
      if [ -n "$token" ]; then expect_status "customer sees the order" 200 "/api/q/$token"; fi
    fi
  fi
else
  echo "skip  signed-in checks (set SMOKE_ADMIN_PASSWORD to run them)"
fi

echo
if [ "$failures" -eq 0 ]; then echo "smoke test passed"; else echo "smoke test FAILED: $failures check(s)"; exit 1; fi
