from datetime import datetime
from typing import Any, Optional

from ninja import Schema


class ProjectListOut(Schema):
    id: int
    name: str
    updated_at: datetime


class ProjectOut(Schema):
    id: int
    name: str
    graph: Any
    updated_at: datetime


class ProjectIn(Schema):
    name: str
    graph: Any = {}


class ProjectUpdateIn(Schema):
    name: Optional[str] = None
    graph: Optional[Any] = None
