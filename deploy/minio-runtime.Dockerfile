FROM debian:trixie
COPY ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY minio /usr/local/bin/minio
EXPOSE 9000 9001
ENTRYPOINT ["/usr/local/bin/minio"]
