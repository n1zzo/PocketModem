FROM ghcr.io/flatpak/flatpak-builder:latest
LABEL maintainer="PocketModem Team"

# Install build dependencies
RUN flatpak install flathub org.gnome.Platform//50 org.gnome.Sdk//50 -y && \
    flatpak install flathub org.freedesktop.Sdk.Extension.rust-stable -y

WORKDIR /app

# Copy project
COPY . /app

# Build
ENTRYPOINT ["flatpak-builder", "--force-clean", "build", "flatpak/org.pocketmodem.pocket-modem.yml"]