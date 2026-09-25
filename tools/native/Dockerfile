# Host toolchain for the native test suites and the mutation tool, the same as the
# ubuntu-24.04 CI runner (g++ 13, CMake, Python 3). Used by tools/native/docker.sh.
# Ninja, mold and ccache make the per-mutant rebuilds and the worker setup of mutate.py fast.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake g++ make ninja-build mold ccache python3 procps ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
