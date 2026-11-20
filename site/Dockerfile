# Pinned: never a floating tag for prod (nginx CVE-2026-123007 fixed in 1.30.7 / 1.31.9). Bump deliberately.
FROM nginx:1.31.9-alpine
# COPY-only (no RUN): the image is built for linux/arm64 on an x86_64 runner without emulation
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY dist/ /usr/share/nginx/html/
