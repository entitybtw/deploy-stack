FROM python:3.12-alpine

ARG PIP_PACKAGES=""

RUN if [ -n "$PIP_PACKAGES" ]; then pip install --no-cache-dir $PIP_PACKAGES; fi

WORKDIR /app
