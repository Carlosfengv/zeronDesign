# syntax=docker/dockerfile:1.7
FROM node:22-bookworm-slim AS builder
WORKDIR /build

COPY packages/shared/package.json packages/shared/package-lock.json ./packages/shared/
RUN npm ci --prefix packages/shared
COPY packages/shared ./packages/shared

COPY apps/web/package.json apps/web/package-lock.json ./apps/web/
RUN npm ci --prefix apps/web
COPY apps/web ./apps/web
RUN npm run build --prefix apps/web

FROM node:22-bookworm-slim
ENV NODE_ENV=production \
    HOSTNAME=0.0.0.0 \
    PORT=3000
WORKDIR /app/apps/web
COPY --from=builder --chown=node:node /build /app
USER node
EXPOSE 3000
CMD ["npm", "start"]
