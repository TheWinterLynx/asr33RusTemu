"""Small test doubles shared by Python characterization tests."""


class ConfigStub:
    def __init__(self, values=None):
        self.values = values or {}

    def get(self, *keys, default=None):
        current = self.values
        for key in keys:
            if not isinstance(current, dict) or key not in current:
                return default
            current = current[key]
        return current


class DataSink:
    def __init__(self):
        self.received = []

    def receive_data(self, data):
        self.received.append(data)


class SendSink:
    def __init__(self):
        self.sent = []

    def send_data(self, data):
        self.sent.append(data)
