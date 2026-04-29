FROM rust:1.78-slim AS build
WORKDIR /workspace
COPY . .
RUN cargo build -p corvid-cli --release

FROM debian:bookworm-slim
WORKDIR /workspace
COPY --from=build /workspace/target/release/corvid /usr/local/bin/corvid
COPY examples/backend/personal_executive_agent examples/backend/personal_executive_agent
COPY std std
EXPOSE 8080
CMD ["corvid", "run", "examples/backend/personal_executive_agent/src/main.cor"]
