from CTFd.models import db, Challenges


class InstanceChallenge(Challenges):
    __mapper_args__ = {"polymorphic_identity": "instance"}
    __tablename__ = "nervctf_instance_challenge"

    id = db.Column(
        db.Integer, db.ForeignKey("challenges.id", ondelete="CASCADE"), primary_key=True
    )

    # Backend
    backend = db.Column(db.String(32), default="docker")

    # Docker
    image = db.Column(db.Text, default="")
    command = db.Column(db.Text, default="")

    # Compose
    compose_file = db.Column(db.Text, default="docker-compose.yml")
    compose_service = db.Column(db.Text, default="")

    # LXC
    lxc_image = db.Column(db.Text, default="")

    # Vagrant
    vagrantfile = db.Column(db.Text, default="")

    # Common
    internal_port = db.Column(db.Integer, default=1337)
    connection = db.Column(db.String(32), default="nc")
    timeout_minutes = db.Column(db.Integer, default=45)
    max_renewals = db.Column(db.Integer, default=3)

    # Flag
    flag_mode = db.Column(db.String(16), default="static")
    flag_prefix = db.Column(db.Text, default="")
    flag_suffix = db.Column(db.Text, default="")
    random_flag_length = db.Column(db.Integer, default=16)

    # Dynamic scoring (optional)
    initial_value = db.Column(db.Integer, default=None, nullable=True)
    minimum_value = db.Column(db.Integer, default=None, nullable=True)
    decay_value = db.Column(db.Integer, default=None, nullable=True)
    decay_function = db.Column(db.String(32), default="linear", nullable=True)
