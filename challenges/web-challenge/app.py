from flask import Flask, request, Response
import os

app = Flask(__name__)
flag = os.environ.get('FLAG', 'flag{example_flag_1}')

@app.route('/')
def index():
    return """
    <h1>Welcome to the Web Challenge!</h1>
    <p>Can you find the hidden flag?</p>
    <p>Hint: Check the response headers</p>
    """

@app.route('/secret')
def secret():
    response = Response("Nothing to see here...")
    response.headers['X-Flag'] = flag
    return response

if __name__ == '__main__':
    app.run(host='0.0.0.0', port=5000)
