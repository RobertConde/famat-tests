# The app image: the famat server (and CLI) with LibreOffice for PDF
# conversion. Deployment (Quadlet, secrets, health check) is #33.
#
#   podman build -t famat -f Containerfile .
#   podman run --rm -p 8080:8080 -e DATABASE_URL=… -e S3_ENDPOINT=… … famat
#
# Configuration is the environment the README lists.

# ------------------------------------------------------------------ build
FROM docker.io/library/rust:1-trixie AS build
RUN rustup target add wasm32-unknown-unknown \
 && cargo install dioxus-cli --version 0.7.10 --locked
WORKDIR /src
COPY . .
RUN scripts/bundle.sh

# ---------------------------------------------------------------- runtime
FROM docker.io/library/debian:trixie-slim

# LibreOffice without its GUI: Writer for Word, RTF and ODF, Impress for
# PowerPoint. Metric-compatible fonts (Liberation for Times New Roman, Arial
# and Courier New; Carlito for Calibri; Caladea for Cambria) keep page
# breaks where Word put them.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      libreoffice-writer-nogui \
      libreoffice-impress-nogui \
      fonts-liberation \
      fonts-crosextra-carlito \
      fonts-crosextra-caladea \
      fonts-dejavu-core \
      ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# The profile every conversion copies, set up once here so each job skips
# LibreOffice's first-start work. The server turns MathTypeToMath off in
# each copy.
RUN soffice --headless --norestore --nologo \
      -env:UserInstallation=file:///opt/lo-profile-template --terminate_after_init \
 && chmod -R a+rX /opt/lo-profile-template

RUN useradd --system --create-home --home-dir /var/lib/famat --shell /usr/sbin/nologin famat
COPY --from=build /src/dist /app

ENV IP=0.0.0.0 \
    PORT=8080 \
    PDF_PROFILE_TEMPLATE=/opt/lo-profile-template \
    PDF_SOFFICE=soffice \
    DIOXUS_PUBLIC_PATH=/app/public
USER famat
WORKDIR /var/lib/famat
EXPOSE 8080
ENTRYPOINT ["/app/server"]
