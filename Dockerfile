FROM clux/muslrust:stable AS builder
COPY Cargo.* .
COPY src src
RUN --mount=type=cache,target=/volume/target \
    --mount=type=cache,target=/root/.cargo/registry \
    cargo build --release -p uniskai-agent && \
    mv /volume/target/x86_64-unknown-linux-musl/release/uniskai-agent .

# FROM gcr.io/distroless/static:nonroot
FROM cgr.dev/chainguard/static
COPY --from=builder --chown=nonroot:nonroot /volume/uniskai-agent /app/
EXPOSE 8080
ENTRYPOINT ["/app/uniskai-agent"]
