FROM rust:1.78-slim AS build
WORKDIR /workspace
COPY . .
RUN cargo build -p corvid-cli --release

FROM debian:bookworm-slim
WORKDIR /workspace
COPY --from=build /workspace/target/release/corvid /usr/local/bin/corvid
COPY examples/backend/shared_app_template examples/backend/shared_app_template
EXPOSE 8080
CMD ["corvid", "run", "examples/backend/shared_app_template/src/main.cor"]
