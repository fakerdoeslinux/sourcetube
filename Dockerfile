FROM node:22-alpine

WORKDIR /app

COPY package.json server.mjs ./
COPY public ./public

ENV PORT=8787
ENV APP_ORIGIN=http://localhost:8787
ENV NODE_ENV=production

EXPOSE 8787

CMD ["node", "server.mjs"]
