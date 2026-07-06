"""JWT issuing and verification helpers (PyJWT, HS256, with exp)."""
from datetime import datetime, timedelta, timezone

import jwt
from django.conf import settings


def create_token(user) -> str:
    now = datetime.now(timezone.utc)
    payload = {
        "sub": str(user.pk),
        "email": user.email,
        "iat": int(now.timestamp()),
        "exp": int((now + timedelta(seconds=settings.JWT_EXP_SECONDS)).timestamp()),
    }
    return jwt.encode(payload, settings.SECRET_KEY, algorithm=settings.JWT_ALGORITHM)


def decode_token(token: str) -> dict:
    """Decode/verify a token. Raises jwt.PyJWTError subclasses on failure."""
    return jwt.decode(
        token,
        settings.SECRET_KEY,
        algorithms=[settings.JWT_ALGORITHM],
    )
