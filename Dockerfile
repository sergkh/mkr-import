FROM node:24-alpine AS runner
WORKDIR /app
ENV NODE_ENV=production
EXPOSE 3000
COPY package.json .
RUN npm install
COPY /index.js .
CMD ["node", "index.js"]