FROM node:22-bookworm

RUN mkdir -p /opt/anydesign/bootstrap /workspace/project

COPY base/workspace-init.sh /opt/anydesign/bootstrap/workspace-init.sh
COPY base/workspace-channel-server.js /opt/anydesign/bootstrap/workspace-channel-server.js

RUN chmod +x \
  /opt/anydesign/bootstrap/workspace-init.sh \
  /opt/anydesign/bootstrap/workspace-channel-server.js

WORKDIR /workspace/project
EXPOSE 3001

CMD ["/bin/sh", "-lc", "/opt/anydesign/bootstrap/workspace-init.sh && exec node /opt/anydesign/bootstrap/workspace-channel-server.js"]
