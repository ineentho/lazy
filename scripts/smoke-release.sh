#!/usr/bin/env bash
set -eu

binary="${1:?usage: smoke-release.sh BINARY VERSION}"
version="${2:?usage: smoke-release.sh BINARY VERSION}"
binary_dir="$(cd "$(dirname "$binary")" && pwd)"
binary="$binary_dir/$(basename "$binary")"

test "$($binary --version)" = "lazy $version"
$binary --help | grep -q "On-demand dev process activation"

test_home="$(mktemp -d)"
daemon_pid=""
cleanup() {
  if [ -n "$daemon_pid" ]; then
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  rm -rf "$test_home"
}
trap cleanup EXIT INT TERM

HOME="$test_home" "$binary" proxy --listen 127.0.0.1:0 >"$test_home/proxy.log" 2>&1 &
daemon_pid=$!

attempt=0
while [ ! -S "$test_home/.lazy/lazy.sock" ]; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 100 ]; then
    cat "$test_home/proxy.log" >&2
    echo "lazy proxy did not start" >&2
    exit 1
  fi
  sleep 0.05
done

test "$(HOME="$test_home" "$binary" status)" = "no services registered"
