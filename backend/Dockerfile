# Backend (Rust API). Build context is the repository root:
#   docker build -f backend/Dockerfile -t posmini-backend .
FROM rust:1-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY backend ./backend
# The all-in-one mode embeds the web pages, so they must be present at compile time.
COPY frontend/public ./frontend/public
RUN cargo build --release -p posmini

FROM debian:bookworm-slim
# Thai font for drawing queue tickets / receipts as pictures (see backend/src/raster.rs).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates fonts-thai-tlwg \
 && rm -rf /var/lib/apt/lists/* \
 && test -f /usr/share/fonts/truetype/tlwg/Loma.ttf \
 && test -f /usr/share/fonts/truetype/tlwg/Loma-Bold.ttf
COPY --from=build /app/target/release/posmini /usr/local/bin/posmini
# The database lives in /data; on Railway a volume is mounted over it.
RUN mkdir -p /data

# API only: the pages come from the separate frontend service. Mount a volume on /data.
ENV BIND_ADDR=[::]:8080 \
    SERVE_UI=false \
    TRUST_PROXY=true \
    COOKIE_SECURE=true \
    DATABASE_URL=sqlite:///data/posmini.db?mode=rwc
EXPOSE 8080
CMD ["posmini"]
