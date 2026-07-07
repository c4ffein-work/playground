from typing import Any, List, Optional

from ninja import Schema


class MessagesIn(Schema):
    model: str
    max_tokens: int
    messages: List[Any]
    tools: Optional[List[Any]] = None
    system: Optional[Any] = None
