FROM debian:trixie
RUN apt-get update -qq && apt-get install -y --no-install-recommends curl ca-certificates && rm -rf /var/lib/apt/lists/*
COPY ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY minio /usr/local/bin/minio
EXPOSE 9000 9001
ENTRYPOINT ["/usr/local/bin/minio"]
