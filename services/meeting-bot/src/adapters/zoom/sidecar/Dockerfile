# Builder stage for the Zoom sidecar. The meeting-bot image (owned by the runner
# lane) copies /opt/allternit/zoom from here:
#
#   COPY --from=zoom-sidecar /opt/allternit/zoom /opt/allternit/zoom
#
# and installs the runtime packages listed under "runtime" below. BASE_IMAGE must
# match the meeting-bot runtime base so glibc and libstdc++ agree.
#
# Build (the SDK tarball is NOT in git; put it in vendor/ first, see README.md):
#   docker build -f Dockerfile --build-arg SDK_TARBALL=vendor/zoom-meeting-sdk-linux.tar.xz -t zoom-sidecar .
ARG BASE_IMAGE=ubuntu:22.04
FROM ${BASE_IMAGE} AS zoom-sidecar
ARG DEBIAN_FRONTEND=noninteractive
ARG SDK_TARBALL=vendor/zoom-meeting-sdk-linux.tar.xz

# build
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential cmake pkgconf ca-certificates xz-utils \
      libglib2.0-dev nlohmann-json3-dev \
    # runtime (also needed in the meeting-bot image)
      libglib2.0-0 libdbus-1-3 libgbm1 libgl1 libgl1-mesa-dri libxfixes3 \
      libx11-xcb1 libxcb-xfixes0 libxcb-shape0 libxcb-shm0 libxcb-randr0 \
      libxcb-image0 libxcb-keysyms1 libxcb-xtest0 \
      pulseaudio pulseaudio-utils \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY CMakeLists.txt build.sh ./
COPY src ./src
COPY ${SDK_TARBALL} /tmp/zoom-sdk.tar
RUN bash build.sh /tmp/zoom-sdk.tar /opt/allternit/zoom && rm -rf /tmp/zoom-sdk.tar /src/build /src/vendor
