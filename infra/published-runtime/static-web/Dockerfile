ARG BASE_IMAGE=registry-1.docker.io/library/nginx@sha256:53cadfbebeffa241f12333cf8a63f3c6553eedad8b9f8296de89e32c566a5caa
FROM ${BASE_IMAGE}

USER root
COPY nginx.conf /etc/nginx/nginx.conf
COPY public/ /srv/www/
COPY metadata/ /.anydesign/
RUN chown -R 101:101 /srv/www /.anydesign /var/cache/nginx \
    && chmod -R u=rwX,go=rX /srv/www /.anydesign \
    && chmod 0444 /etc/nginx/nginx.conf

USER 101:101
EXPOSE 8080
STOPSIGNAL SIGQUIT
CMD ["nginx", "-g", "daemon off;"]
