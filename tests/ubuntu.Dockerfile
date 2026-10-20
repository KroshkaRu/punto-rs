ARG UBUNTU=22.04
FROM ubuntu:${UBUNTU}
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get -o Acquire::http::Timeout=30 update -qq && apt-get install -y --no-install-recommends \
    build-essential pkg-config python3-gi gir1.2-ibus-1.0 gir1.2-gtk-3.0 ibus ibus-gtk3 \
    hunspell-ru hunspell-en-us im-config at-spi2-core gir1.2-atspi-2.0 x11-xkb-utils dbus-x11 xvfb xauth xdotool libegl1 libgl1 \
    libxkbcommon0 libxkbcommon-x11-0 libxi6 libxrandr2 libxcursor1 libxinerama1 \
    fonts-dejavu-core ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /work
