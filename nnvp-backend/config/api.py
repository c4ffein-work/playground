"""Single NinjaAPI instance mounting all app routers under /api."""
from ninja import NinjaAPI

from accounts.api import router as accounts_router
from assistant.api import router as assistant_router
from projects.api import router as projects_router

api = NinjaAPI(title="NNVP Backend API", version="1.0.0")

api.add_router("/auth", accounts_router, tags=["auth"])
api.add_router("/projects", projects_router, tags=["projects"])
api.add_router("/assistant", assistant_router, tags=["assistant"])
