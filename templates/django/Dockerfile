FROM python:3.12-alpine
RUN apk add --no-cache gcc musl-dev postgresql-dev
WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY . .
CMD ["sh", "-c", "python manage.py migrate && gunicorn ${APP_MODULE:-config.wsgi:application} --bind 0.0.0.0:8000"]
