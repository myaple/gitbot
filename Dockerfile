FROM rust:latest AS builder
WORKDIR /usr/src/gitbot
COPY . .
RUN cargo build --release

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /usr/src/gitbot/target/release/gitbot /usr/local/bin/gitbot
EXPOSE 8080
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/gitbot"] 