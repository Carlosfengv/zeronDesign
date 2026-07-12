ARG BASE_IMAGE=registry-1.docker.io/library/nginx@sha256:0d3b80406a13a767339fbe2f41406d6c7da727ab89cf8fae399e81f780f814d1
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
