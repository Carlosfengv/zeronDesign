# zeronDesign Product Web

Phase B Product/BFF skeleton. The browser calls only this Next.js application;
Runtime calls stay server-side and use `@zerondesign/shared` contracts.

```bash
cp .env.example .env.local
npm install
npm run dev
```

`ZERONDESIGN_DEV_USER_ID` is accepted only outside production. Production fails
closed unless a non-expired HMAC-signed `zerondesign_session` cookie is supplied;
the identity integration must issue it with `issueSession()` and a session secret
of at least 32 bytes. `RUNTIME_PUBLIC_PRINCIPAL_TOKEN` is suitable only for a
local project-scoped smoke test; production must mint a short-lived token for
the authenticated principal, project and requested operation.
