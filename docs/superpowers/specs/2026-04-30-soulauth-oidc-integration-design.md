# SoulAuth OIDC Integration Design

Date: 2026-04-30

## Goal

Use SoulAuth as the long-term identity provider for SoulBook. SoulBook should stop owning Google OAuth directly and instead act as an OpenID Connect client. SoulAuth owns Google login, browser login state, OIDC tokens, and user identity. SoulBook owns its application session, document permissions, and local user mapping.

## Current State

SoulBook currently has a direct Google OAuth flow under `/api/docs/auth/google`. It exchanges the Google authorization code, creates or finds `local_user`, signs a SoulBook JWT, and redirects through `/sso` so the frontend can store the token.

SoulAuth currently has OIDC route structure under `/api/oidc`, including discovery, authorize, token, userinfo, and logout. The OIDC implementation is not production-ready yet: key database operations for clients, authorization codes, access tokens, refresh tokens, and user lookup are still unimplemented. The browser authorization endpoint also checks only an `Authorization: Bearer` header, which does not work for a normal browser redirect-based OIDC login.

SoulBookFront currently links the Google login button to SoulBook's direct Google OAuth endpoint. Vercel rewrites API traffic to the server, but SoulAuth is not exposed through a stable public `/auth/` prefix yet.

## Recommended Architecture

SoulAuth becomes the identity provider and OIDC issuer. SoulBook becomes a confidential OIDC client.

Public routing should expose SoulAuth behind a stable HTTPS prefix:

```text
https://soul-book-front.vercel.app/auth/...
```

On the server, Nginx proxies `/auth/` to SoulAuth on `127.0.0.1:8080`. On Vercel, `/auth/:path*` rewrites to the server `/auth/:path*` path. This keeps browser-visible OAuth and OIDC URLs under HTTPS and avoids exposing raw port `8080`.

SoulAuth should use this public issuer URL:

```text
https://soul-book-front.vercel.app/auth
```

SoulBook should use OIDC client settings:

```text
SOULAUTH_ISSUER=https://soul-book-front.vercel.app/auth
SOULAUTH_CLIENT_ID=<generated-client-id>
SOULAUTH_CLIENT_SECRET=<generated-client-secret>
SOULAUTH_REDIRECT_URI=https://soul-book-front.vercel.app/api/docs/auth/soulauth/callback
SOULAUTH_POST_LOGOUT_REDIRECT_URI=https://soul-book-front.vercel.app/docs/login
```

Google Console should point to SoulAuth only:

```text
https://soul-book-front.vercel.app/auth/api/auth/callback/google
```

## Login Flow

1. User clicks "Use Google / SoulAuth" on SoulBookFront.
2. SoulBookFront navigates to `/api/docs/auth/soulauth/start`.
3. SoulBook creates `state`, `nonce`, and PKCE verifier/challenge.
4. SoulBook stores the short-lived login transaction server-side or in a signed encrypted state token.
5. SoulBook redirects to SoulAuth `/api/oidc/authorize`.
6. SoulAuth checks its browser login session cookie.
7. If no SoulAuth session exists, SoulAuth redirects to its login page.
8. User chooses Google login.
9. Google redirects to SoulAuth `/api/auth/callback/google`.
10. SoulAuth creates or updates the SoulAuth user, creates a browser session cookie, and resumes the original OIDC authorize request.
11. SoulAuth validates the OIDC client and redirect URI, creates an authorization code, and redirects back to SoulBook callback.
12. SoulBook exchanges the code at SoulAuth `/api/oidc/token` with `client_id`, `client_secret`, and PKCE verifier.
13. SoulBook calls SoulAuth `/api/oidc/userinfo` with the OIDC access token.
14. SoulBook maps the SoulAuth subject to a local `local_user`, creating one if needed.
15. SoulBook signs its own application JWT and redirects through existing `/sso?token=...&next=...`.
16. SoulBookFront stores the SoulBook JWT using the existing storage shape and enters the app.

## SoulAuth Changes

SoulAuth must complete its OIDC provider implementation.

Required changes:

- Implement OIDC client lookup and validation from the database.
- Implement authorization code persistence, one-time use, expiration, and redirect URI binding.
- Implement access token persistence and lookup for `/userinfo`.
- Implement refresh token persistence if refresh tokens remain enabled.
- Implement user lookup by SoulAuth user id for ID token and userinfo claims.
- Add browser login session support using secure HTTP-only cookies.
- Change `/api/oidc/authorize` so a normal browser request can authenticate through the SoulAuth session cookie instead of requiring an `Authorization` header.
- Preserve and resume the original authorize request after Google login.
- Add a minimal login page or redirect endpoint that lets users start Google login when OIDC authorization requires authentication.
- Ensure issuer and discovery endpoints use the public `/auth` issuer URL, not internal `127.0.0.1:8080` or bare server IP.

Token signing can initially remain HS256 if SoulBook validates tokens only through back-channel token exchange and `/userinfo`. A later hardening step should migrate SoulAuth OIDC ID tokens to RS256 with a real JWKS response.

## SoulBook Changes

SoulBook should add a new SoulAuth OIDC route module, separate from the existing direct Google OAuth module.

Required routes:

```text
GET /api/docs/auth/soulauth/start
GET /api/docs/auth/soulauth/callback
POST /api/docs/auth/logout
```

The start route creates the OIDC authorization request. The callback route validates `state`, exchanges the code, fetches userinfo, maps the identity, signs a SoulBook JWT, and redirects through `/sso`.

SoulBook should map external identities by stable SoulAuth subject, not only by email. The local user schema should support:

```text
provider = "soulauth"
external_subject = <SoulAuth sub>
email = <SoulAuth email>
```

If an existing local user has the same verified email but no external subject, SoulBook may link it to SoulAuth during first login. If the email is missing or unverified, login should fail with a clear error.

The existing direct Google route can remain during rollout as a fallback, but the frontend should switch to the SoulAuth route once the new flow passes verification.

## SoulBookFront Changes

SoulBookFront should stop linking the main Google login button to `/api/docs/auth/google/start`.

The primary login action should point to:

```text
/api/docs/auth/soulauth/start
```

The existing `/sso` localStorage bridge remains unchanged. This avoids a large frontend auth-state rewrite and limits the change to the login entry point.

Vercel should add a rewrite for:

```text
/auth/:path* -> http://47.236.185.219/auth/:path*
```

## Deployment Changes

Nginx should expose SoulAuth through `/auth/` and preserve forwarded headers:

```nginx
location /auth/ {
    proxy_pass http://127.0.0.1:8080/;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
}
```

SoulAuth environment should set its public application or issuer URL to:

```text
APP_URL=https://soul-book-front.vercel.app/auth
OAUTH_REDIRECT_URL=https://soul-book-front.vercel.app/auth/api/auth/callback
```

SoulBook environment should add the SoulAuth OIDC client variables and keep its own `JWT_SECRET`.

## Error Handling

SoulAuth should redirect OIDC errors back to the validated client redirect URI when possible, with `error`, `error_description`, and original `state`.

SoulBook callback should handle:

- Missing or invalid `state`.
- User-cancelled login.
- Code exchange failure.
- Userinfo failure.
- Missing email.
- Unverified email.
- Local user linking conflicts.

Failures that happen after returning to SoulBook should render a user-readable login error or redirect to `/docs/login?error=...`.

## Security Requirements

- Use authorization code flow with PKCE.
- Validate `state` and `nonce`.
- Validate redirect URIs exactly against registered OIDC client configuration.
- Use short-lived authorization codes and mark codes as used after exchange.
- Store browser session cookies as `HttpOnly`, `Secure`, and `SameSite=Lax`.
- Never expose `client_secret` to SoulBookFront.
- Keep Google client secret only in SoulAuth.
- Keep SoulBook JWT signing separate from SoulAuth tokens.
- Prefer verified email for account linking.

## Rollout Plan

1. Implement and test SoulAuth OIDC persistence and browser session login.
2. Register a SoulBook OIDC client in SoulAuth.
3. Add SoulBook OIDC client routes and identity mapping.
4. Add Vercel and Nginx `/auth/` routing.
5. Switch SoulBookFront login button to SoulAuth.
6. Deploy SoulAuth first, then SoulBook, then SoulBookFront.
7. Verify direct login, refresh page persistence, logout, and old Google fallback.
8. After stable operation, remove or hide SoulBook's direct Google login route.

## Verification

Local and server verification should include:

- SoulAuth discovery returns the public issuer and endpoints.
- SoulAuth authorize redirects unauthenticated browsers to login.
- Google login creates a SoulAuth browser session.
- SoulAuth authorize returns an authorization code to SoulBook.
- SoulBook callback exchanges code successfully.
- SoulBook maps or creates local user using SoulAuth subject.
- `/sso` stores the SoulBook JWT and frontend enters `/docs/`.
- Existing document APIs work with the new SoulBook JWT.
- Logout clears SoulBook frontend state and, if enabled, SoulAuth session.

## Deferred Hardening

The first implementation can use HS256 internally if SoulBook relies on SoulAuth `/userinfo` for identity. A later hardening task should move OIDC ID token signing to RS256 and return a populated JWKS document.

Longer term, use a dedicated domain such as `https://auth.soulbook.example` instead of a Vercel path prefix. A dedicated auth domain reduces rewrite complexity and matches common OIDC deployments better.
