from django.contrib.auth import authenticate
from django.db import IntegrityError
from ninja import Router
from ninja.errors import HttpError

from accounts.auth import JWTAuth
from accounts.models import User
from accounts.schemas import AuthOut, LoginIn, RegisterIn, UserOut
from accounts.tokens import create_token

router = Router()


@router.post("/register", response={201: AuthOut}, auth=None)
def register(request, data: RegisterIn):
    email = data.email.strip().lower()
    if not email or not data.password:
        raise HttpError(422, "Email and password are required.")
    try:
        user = User.objects.create_user(email=email, password=data.password)
    except IntegrityError:
        raise HttpError(409, "A user with that email already exists.")
    token = create_token(user)
    return 201, {"token": token, "user": {"id": user.id, "email": user.email}}


@router.post("/login", response={200: AuthOut}, auth=None)
def login(request, data: LoginIn):
    email = data.email.strip().lower()
    # Our USERNAME_FIELD is email, so authenticate() takes username=email.
    user = authenticate(request, username=email, password=data.password)
    if user is None:
        raise HttpError(401, "Invalid email or password.")
    token = create_token(user)
    return 200, {"token": token, "user": {"id": user.id, "email": user.email}}


@router.get("/me", response={200: UserOut}, auth=JWTAuth())
def me(request):
    user = request.auth
    return 200, {"id": user.id, "email": user.email}
