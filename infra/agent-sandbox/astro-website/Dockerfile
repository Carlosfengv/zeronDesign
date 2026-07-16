ARG SANDBOX_BASE_IMAGE=node:22-bookworm@sha256:5647be709086c696ff32edaaf1c70cd26d1da6ab2b39c32f3c7b4c4a31957e37
FROM ${SANDBOX_BASE_IMAGE}

RUN mkdir -p /opt/anydesign/bootstrap /workspace/project

COPY base/workspace-init.sh /opt/anydesign/bootstrap/workspace-init.sh
COPY base/workspace-channel-server.js /opt/anydesign/bootstrap/workspace-channel-server.js
COPY base/static-preview-server.js /opt/anydesign/bootstrap/static-preview-server.js

RUN chmod +x \
  /opt/anydesign/bootstrap/workspace-init.sh \
  /opt/anydesign/bootstrap/workspace-channel-server.js \
  /opt/anydesign/bootstrap/static-preview-server.js

WORKDIR /workspace/project
EXPOSE 3001 4321

CMD ["/bin/sh", "-lc", "/opt/anydesign/bootstrap/workspace-init.sh && exec node /opt/anydesign/bootstrap/workspace-channel-server.js"]
