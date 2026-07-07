from typing import List

from django.shortcuts import get_object_or_404
from ninja import Router

from accounts.auth import JWTAuth
from projects.models import Project
from projects.schemas import (
    ProjectIn,
    ProjectListOut,
    ProjectOut,
    ProjectUpdateIn,
)

# Every route in this router requires a valid Bearer JWT.
router = Router(auth=JWTAuth())


def _owned(request):
    return Project.objects.filter(owner=request.auth)


@router.get("", response={200: List[ProjectListOut]})
def list_projects(request):
    # List WITHOUT the full graph blob.
    return 200, list(_owned(request).values("id", "name", "updated_at"))


@router.post("", response={201: ProjectOut})
def create_project(request, data: ProjectIn):
    project = Project.objects.create(
        owner=request.auth,
        name=data.name,
        graph=data.graph if data.graph is not None else {},
    )
    return 201, project


@router.get("/{int:project_id}", response={200: ProjectOut})
def get_project(request, project_id: int):
    # 404 if the project does not exist OR is not owned by the caller.
    project = get_object_or_404(_owned(request), pk=project_id)
    return 200, project


@router.put("/{int:project_id}", response={200: ProjectOut})
def update_project(request, project_id: int, data: ProjectUpdateIn):
    project = get_object_or_404(_owned(request), pk=project_id)
    payload = data.dict(exclude_unset=True)
    if "name" in payload and payload["name"] is not None:
        project.name = payload["name"]
    if "graph" in payload:
        project.graph = payload["graph"]
    project.save()
    return 200, project


@router.delete("/{int:project_id}", response={204: None})
def delete_project(request, project_id: int):
    project = get_object_or_404(_owned(request), pk=project_id)
    project.delete()
    return 204, None
