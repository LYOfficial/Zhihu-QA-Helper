FROM python:3.12-alpine

WORKDIR /app
COPY server.py .
USER 10001:10001
EXPOSE 33111
CMD ["python", "-B", "server.py", "--host", "0.0.0.0"]
