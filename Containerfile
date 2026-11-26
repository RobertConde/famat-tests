# The app image: the famat server (and CLI) with LibreOffice for PDF
# conversion. Deployment (Docker Compose, secrets, health check) is in
# deploy/ and docs/deploy/.
#
#   podman build -t famat -f Containerfile .
#   podman run --rm -p 8080:8080 -e DATABASE_URL=… -e S3_ENDPOINT=… … famat
#
# Configuration is the environment the README lists.
#
# CI builds the release bundle first, runs the HTTP suite against it, and then
# puts that same bundle in the image instead of building it again here:
#
#   podman build --build-context bundle=dist -t famat -f Containerfile .
#
# A named build context replaces the `bundle` stage below, so its directory
# must look like dist/: `server` with `public/` beside it.

# ------------------------------------------------------------------ build
FROM docker.io/library/rust:1-trixie AS build
RUN rustup target add wasm32-unknown-unknown \
 && cargo install dioxus-cli --version 0.7.10 --locked
WORKDIR /src
COPY . .
RUN scripts/bundle.sh

# The bundle alone (dist/'s contents at /), replaceable with --build-context.
FROM scratch AS bundle
COPY --from=build /src/dist /

# ---------------------------------------------------------------- runtime
FROM docker.io/library/debian:trixie-slim

# LibreOffice without its GUI: Writer for Word, RTF and ODF, Impress for
# PowerPoint. Metric-compatible fonts (Liberation for Times New Roman, Arial
# and Courier New; Carlito for Calibri; Caladea for Cambria) keep page
# breaks where Word put them. curl is for the health check (the app
# service's healthcheck in deploy/compose.yaml).
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      libreoffice-writer-nogui \
      libreoffice-impress-nogui \
      fonts-liberation \
      fonts-crosextra-carlito \
      fonts-crosextra-caladea \
      fonts-dejavu-core \
      ca-certificates \
      curl \
 && rm -rf /var/lib/apt/lists/*

# The profile every conversion copies, set up once here so each job skips
# LibreOffice's first-start work. The server turns MathTypeToMath off in
# each copy.
RUN soffice --headless --norestore --nologo \
      -env:UserInstallation=file:///opt/lo-profile-template --terminate_after_init \
 && chmod -R a+rX /opt/lo-profile-template

RUN useradd --system --create-home --home-dir /var/lib/famat --shell /usr/sbin/nologin famat
COPY --from=bundle / /app

LABEL org.opencontainers.image.source="https://github.com/RobertConde/famat-tests" \
      org.opencontainers.image.description="FAMAT Test Bank server"

ENV IP=0.0.0.0 \
    PORT=8080 \
    PDF_PROFILE_TEMPLATE=/opt/lo-profile-template \
    PDF_SOFFICE=soffice \
    DIOXUS_PUBLIC_PATH=/app/public
USER famat
WORKDIR /var/lib/famat
EXPOSE 8080
ENTRYPOINT ["/app/server"]
