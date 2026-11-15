FROM rust:1.99.0-bookworm@sha256:114c7a4425406451c2866b6aafe69fe29b1b298832db1277d411ac73c82d04d6 AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --bin soze

FROM debian:12.12-slim@sha256:d5d3f9c23164ea16f31852f95bd5959aad1c5e854332fe00f7b3a20fcc9f635c
# Freeze OS dependencies too; this snapshot is over two weeks old.
RUN rm -f /etc/apt/sources.list.d/debian.sources \
    && printf '%s\n' \
       'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/20261015T000000Z bookworm main' \
       'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/20261015T000000Z bookworm-security main' \
       > /etc/apt/sources.list \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y \
       systemd systemd-sysv dbus-user-session procps python3 util-linux \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --create-home --shell /bin/sh sozetest

COPY --from=build /build/target/debug/soze /opt/soze/soze
COPY scripts/service.sh /opt/soze/scripts/service.sh
COPY tests/systemd/inside.sh tests/systemd/test_service.py /opt/soze/tests/systemd/
ENV container=docker
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
