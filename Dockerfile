FROM rust:slim AS builder

WORKDIR /crux

COPY ./ .

RUN cargo build -p crux-server --release


FROM debian:stable-slim AS server

WORKDIR /crux

COPY --from=builder /crux/target/release/crux-server ./

CMD [ "./crux-server" ]