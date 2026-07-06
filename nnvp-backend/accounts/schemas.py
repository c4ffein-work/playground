from ninja import Schema


class RegisterIn(Schema):
    email: str
    password: str


class LoginIn(Schema):
    email: str
    password: str


class UserOut(Schema):
    id: int
    email: str


class AuthOut(Schema):
    token: str
    user: UserOut
