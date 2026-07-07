"""Env-driven CORS middleware for the SPA (dependency-free, no django-cors-headers).

Behavior is controlled by settings.CORS_ALLOWED_ORIGINS:
- contains "*" (the default only when DEBUG=True): wildcard — any origin allowed.
- non-empty list: the request's Origin header must exactly match an entry
  (scheme://host[:port]); matches get the origin echoed back plus Vary: Origin.
- empty list (the default when DEBUG=False): no CORS headers at all — browsers
  block cross-origin reads until CORS_ALLOWED_ORIGINS is configured.
"""
from django.conf import settings
from django.http import HttpResponse
from django.utils.cache import patch_vary_headers


class CorsMiddleware:
    def __init__(self, get_response):
        self.get_response = get_response

    def __call__(self, request):
        if request.method == "OPTIONS" and "HTTP_ACCESS_CONTROL_REQUEST_METHOD" in request.META:
            response = HttpResponse(status=204)
        else:
            response = self.get_response(request)

        allowed = settings.CORS_ALLOWED_ORIGINS
        origin = request.META.get("HTTP_ORIGIN")
        allow_origin = None
        if "*" in allowed:
            allow_origin = "*"
        elif origin and origin in allowed:
            allow_origin = origin
            patch_vary_headers(response, ("Origin",))

        if allow_origin:
            response["Access-Control-Allow-Origin"] = allow_origin
            response["Access-Control-Allow-Methods"] = "GET, POST, PUT, PATCH, DELETE, OPTIONS"
            response["Access-Control-Allow-Headers"] = (
                "Authorization, Content-Type, X-Requested-With"
            )
            response["Access-Control-Max-Age"] = "86400"
        return response
