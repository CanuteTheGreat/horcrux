FROM nginx:alpine
# COPY-only (no RUN): the image is built for linux/arm64 on an x86_64 runner without emulation
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY dist/ /usr/share/nginx/html/
