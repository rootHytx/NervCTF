FROM python:3.10-slim

WORKDIR /app

COPY . .

RUN pip install --no-cache-dir flask

ENV FLAG=flag{example_flag_1}

EXPOSE 5000

CMD ["python", "app.py"]
