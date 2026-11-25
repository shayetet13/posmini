# Frontend: the static pages, plus a reverse proxy that sends /api, /events, ... to the backend.
# Because the browser only ever talks to this service, cookies and live updates (SSE) just work.
#   docker build -f frontend/Dockerfile -t posmini-frontend .
FROM caddy:2-alpine
COPY frontend/Caddyfile /etc/caddy/Caddyfile
COPY frontend/public /srv
ENV PORT=8080 \
    BACKEND_URL=http://backend.railway.internal:8080
CMD ["caddy", "run", "--config", "/etc/caddy/Caddyfile", "--adapter", "caddyfile"]
