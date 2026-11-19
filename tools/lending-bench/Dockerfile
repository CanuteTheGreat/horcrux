FROM debian:bookworm
RUN apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq fio iperf3 nvme-cli iproute2 kmod procps netcat-openbsd python3 >/dev/null && apt-get clean
