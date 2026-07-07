"""Ninja auth class validating the Bearer JWT and setting request.auth to the user."""
import jwt
from ninja.security import HttpBearer

from accounts.models import User
from accounts.tokens import decode_token


class JWTAuth(HttpBearer):
    """Validates a Bearer token; on success returns the User (becomes request.auth)."""

    def authenticate(self, request, token):
        try:
            payload = decode_token(token)
        except jwt.PyJWTError:
            return None
        user_id = payload.get("sub")
        if not user_id:
            return None
        try:
            user = User.objects.get(pk=user_id, is_active=True)
        except (User.DoesNotExist, ValueError):
            return None
        request.auth = user
        return user
