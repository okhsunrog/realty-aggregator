#!/usr/bin/env bash
set -euo pipefail
source_revision=7aac2a2c5b7c882e68c1ce017d8256be2feea27f
build_root=${MINIO_BUILD_ROOT:-/home/okhsunrog/tmp_zfs/codex_tmp/realty-minio}
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
mkdir -p "$build_root"/{bin,go-cache,go-mod,tmp,image}
export TMPDIR="$build_root/tmp" GOBIN="$build_root/bin" GOCACHE="$build_root/go-cache" GOMODCACHE="$build_root/go-mod" CGO_ENABLED=0
go install "github.com/minio/minio@$source_revision"
install -m 755 "$GOBIN/minio" "$build_root/image/minio"
install -m 644 /etc/ssl/certs/ca-certificates.crt "$build_root/image/ca-certificates.crt"
install -m 644 "$script_dir/minio-runtime.Dockerfile" "$build_root/image/Dockerfile"
docker build --tag realty-minio:7aac2a2c5b7c "$build_root/image"
