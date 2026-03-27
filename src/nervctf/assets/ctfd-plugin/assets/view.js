CTFd._internal.challenge.data = undefined;
CTFd._internal.challenge.renderer = null;
CTFd._internal.challenge.preRender = function () {};
CTFd._internal.challenge.render = null;
CTFd._internal.challenge.postRender = function () {};

CTFd._internal.challenge.submit = function (preview) {
  var challenge_id = parseInt(CTFd.lib.$("#challenge-id").val());
  var submission = CTFd.lib.$("#challenge-input").val().trim();
  var body = { challenge_id: challenge_id, submission: submission };
  var params = {};
  if (preview) params["preview"] = true;
  return CTFd.api
    .post_challenge_attempt(params, body)
    .then(function (response) {
      if (response.status === 429) return response;
      if (response.status === 403) return response;
      return response;
    });
};

// ── UI helpers ────────────────────────────────────────────────────────────────

function _makeSpinner() {
  var d = document.createElement("div");
  d.className = "spinner-border text-primary";
  d.setAttribute("role", "status");
  var s = document.createElement("span");
  s.className = "visually-hidden";
  s.textContent = "Loading...";
  d.appendChild(s);
  return d;
}

function resetAlert() {
  var alert = document.getElementById("deployment-info");
  while (alert.firstChild) alert.removeChild(alert.firstChild);
  alert.appendChild(_makeSpinner());
  alert.classList.remove("alert-danger");
  document.getElementById("create-chal").disabled = true;
  document.getElementById("extend-chal").disabled = true;
  document.getElementById("terminate-chal").disabled = true;
  return alert;
}

function enableButtons() {
  document.getElementById("create-chal").disabled = false;
  document.getElementById("extend-chal").disabled = false;
  document.getElementById("terminate-chal").disabled = false;
}

function showCreateBtn() {
  var b = document.getElementById("create-chal");
  if (b) b.classList.remove("d-none");
}
function hideCreateBtn() {
  var b = document.getElementById("create-chal");
  if (b) b.classList.add("d-none");
}
function showUpdateBtns() {
  var e = document.getElementById("extend-chal");
  var t = document.getElementById("terminate-chal");
  if (e) e.classList.remove("d-none");
  if (t) t.classList.remove("d-none");
}
function hideUpdateBtns() {
  var e = document.getElementById("extend-chal");
  var t = document.getElementById("terminate-chal");
  if (e) e.classList.add("d-none");
  if (t) t.classList.add("d-none");
}

function _setAlertText(alert, text) {
  while (alert.firstChild) alert.removeChild(alert.firstChild);
  alert.textContent = text;
}

function _renderWithLabel(connection, parent) {
  var label = document.createElement("strong");
  label.textContent = "Instance Connection";
  parent.append(label, document.createElement("br"));
  renderConnectionInfo(connection, parent);
}

function formatExpiry(timestampMs) {
  var secondsLeft = Math.ceil((timestampMs - Date.now()) / 1000);
  if (secondsLeft < 0) return "Expired";
  if (secondsLeft < 60) return "Expires in " + secondsLeft + " seconds";
  return "Expires in " + Math.ceil(secondsLeft / 60) + " minutes";
}

var _pollTimer = null;

function _pollUntilRunning(challenge_id, alert) {
  if (_pollTimer) clearTimeout(_pollTimer);
  _pollTimer = setTimeout(function () {
    _pollTimer = null;
    fetch("/api/v1/containers/info/" + challenge_id, {
      method: "GET",
      headers: { Accept: "application/json", "CSRF-Token": init.csrfNonce },
    })
      .then(function (r) { return r.json(); })
      .then(function (data) {
        if (data.status === "provisioning") {
          _pollUntilRunning(challenge_id, alert);
        } else {
          view_container_info(challenge_id);
        }
      })
      .catch(function () {
        _pollUntilRunning(challenge_id, alert);
      });
  }, 3000);
}

function renderConnectionInfo(connection, parent) {
  // url_list (multi-port subdomain)
  if (connection.type === "url_list" && connection.urls) {
    connection.urls.forEach(function (item) {
      var a = document.createElement("a");
      a.href = item.url;
      a.textContent = item.url;
      a.target = "_blank";
      a.rel = "noopener noreferrer";
      parent.append(a, document.createElement("br"));
    });
    return;
  }

  if (connection.ports && Object.keys(connection.ports).length > 0) {
    if (connection.type === "tcp" || connection.type === "nc") {
      var code = document.createElement("code");
      code.textContent =
        "nc " +
        connection.host +
        " " +
        Object.values(connection.ports).join(", ");
      parent.append(code);
    } else if (connection.type === "http" || connection.type === "web") {
      Object.entries(connection.ports).forEach(function (entry) {
        var ext = entry[1];
        var a = document.createElement("a");
        a.href = "http://" + connection.host + ":" + ext;
        a.textContent = "http://" + connection.host + ":" + ext;
        a.target = "_blank";
        a.rel = "noopener noreferrer";
        parent.append(a, document.createElement("br"));
      });
    } else if (connection.type === "https") {
      Object.entries(connection.ports).forEach(function (entry) {
        var ext = entry[1];
        var portSuffix = ext && ext !== 443 ? ":" + ext : "";
        var a = document.createElement("a");
        a.href = "https://" + connection.host + portSuffix;
        a.textContent = "https://" + connection.host + portSuffix;
        a.target = "_blank";
        a.rel = "noopener noreferrer";
        parent.append(a, document.createElement("br"));
      });
    } else if (connection.type === "ssh") {
      Object.entries(connection.ports).forEach(function (entry) {
        var ext = entry[1];
        var code = document.createElement("code");
        code.textContent = "ssh -p " + ext + " user@" + connection.host;
        parent.append(code, document.createElement("br"));
      });
    } else {
      Object.entries(connection.ports).forEach(function (entry) {
        var ext = entry[1];
        var code = document.createElement("code");
        code.textContent = connection.host + ":" + ext;
        parent.append(code, document.createElement("br"));
      });
    }
  } else {
    // Single port
    if (connection.type === "tcp" || connection.type === "nc") {
      var code = document.createElement("code");
      code.textContent = "nc " + connection.host + " " + connection.port;
      parent.append(code);
    } else if (connection.type === "ssh") {
      var code = document.createElement("code");
      code.textContent =
        "ssh -p " + connection.port + " user@" + connection.host;
      parent.append(code);
    } else if (connection.type === "url") {
      var a = document.createElement("a");
      var url = connection.url || "https://" + connection.host;
      a.href = url;
      a.textContent = url;
      a.target = "_blank";
      a.rel = "noopener noreferrer";
      parent.append(a);
    } else if (connection.type === "http" || connection.type === "web") {
      var a = document.createElement("a");
      a.href = "http://" + connection.host + ":" + connection.port;
      a.textContent = "http://" + connection.host + ":" + connection.port;
      a.target = "_blank";
      a.rel = "noopener noreferrer";
      parent.append(a);
    } else if (connection.type === "https") {
      var a = document.createElement("a");
      var portSuffix = connection.port && connection.port !== 443
        ? ":" + connection.port : "";
      a.href = "https://" + connection.host + portSuffix;
      a.textContent = "https://" + connection.host + portSuffix;
      a.target = "_blank";
      a.rel = "noopener noreferrer";
      parent.append(a);
    } else {
      var code = document.createElement("code");
      code.textContent = connection.host + ":" + connection.port;
      parent.append(code);
    }
  }

  if (connection.info) {
    if (parent.lastChild && parent.lastChild.tagName !== "BR") {
      parent.append(document.createElement("br"));
    }
    var info = document.createElement("small");
    info.textContent = connection.info;
    parent.append(info);
  }
}

// ── API calls ─────────────────────────────────────────────────────────────────

function view_container_info(challenge_id) {
  var alert = resetAlert();

  fetch("/api/v1/containers/info/" + challenge_id, {
    method: "GET",
    headers: { Accept: "application/json", "CSRF-Token": init.csrfNonce },
  })
    .then(function (r) {
      if (r.status === 401 || r.status === 403 || r.redirected) {
        return { status: "not_logged_in" };
      }
      return r.json();
    })
    .then(function (data) {
      while (alert.firstChild) alert.removeChild(alert.firstChild);

      if (data.status === "not_logged_in") {
        alert.textContent = "Log in to fetch instance.";
        alert.classList.add("alert-info");
        hideUpdateBtns();
        hideCreateBtn();
        return;
      }

      if (data.status === "solved") {
        alert.textContent = "Challenge solved. No instance needed.";
        alert.classList.add("alert-success");
        hideUpdateBtns();
        hideCreateBtn();
      } else if (
        data.status === "not_found" ||
        data.status === "none" ||
        !data.status
      ) {
        alert.textContent = "No Instance Active";
        alert.classList.add("alert-info");
        hideUpdateBtns();
        showCreateBtn();
      } else if (data.status === "provisioning") {
        alert.textContent = "Instance is provisioning\u2026";
        hideCreateBtn();
        showUpdateBtns();
        _pollUntilRunning(challenge_id, alert);
      } else if (data.status === "running") {
        var expires = document.createElement("span");
        expires.textContent = formatExpiry(data.expires_at);
        alert.append(expires, document.createElement("br"));
        _renderWithLabel(data.connection, alert);
        hideCreateBtn();
        showUpdateBtns();
      } else {
        alert.textContent = data.error || "Unknown status";
        alert.classList.add("alert-danger");
        hideUpdateBtns();
        showCreateBtn();
      }
    })
    .catch(function (err) {
      console.error("[Instance] Fetch error:", err);
      _setAlertText(alert, "Error fetching instance info.");
      alert.classList.add("alert-danger");
      showCreateBtn();
    })
    .finally(enableButtons);
}

function container_request(challenge_id) {
  var alert = resetAlert();
  // Replace spinner with static text — compose startup can take >10s and a
  // spinning wheel adds visual noise for something that is expected to be slow.
  while (alert.firstChild) alert.removeChild(alert.firstChild);
  alert.textContent =
    "Spawning instance\u2026 this may take up to 30\u00a0seconds.";

  fetch("/api/v1/containers/request", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json",
      "CSRF-Token": init.csrfNonce,
    },
    body: JSON.stringify({ challenge_id: challenge_id }),
  })
    .then(function (r) {
      return r.json();
    })
    .then(function (data) {
      while (alert.firstChild) alert.removeChild(alert.firstChild);
      if (data.solved) {
        alert.textContent = "Challenge already solved.";
        alert.classList.add("alert-success");
        hideUpdateBtns();
        hideCreateBtn();
      } else if (data.error) {
        alert.textContent = data.error;
        alert.classList.add("alert-danger");
        showCreateBtn();
      } else if (data.status === "provisioning") {
        alert.textContent = "Instance is provisioning\u2026";
        hideCreateBtn();
        showUpdateBtns();
        _pollUntilRunning(challenge_id, alert);
      } else {
        var expires = document.createElement("span");
        expires.textContent = formatExpiry(data.expires_at);
        alert.append(expires, document.createElement("br"));
        _renderWithLabel(data.connection, alert);
        hideCreateBtn();
        showUpdateBtns();
      }
    })
    .catch(function (err) {
      console.error("[Instance] Request error:", err);
      _setAlertText(alert, "Error requesting instance.");
      alert.classList.add("alert-danger");
    })
    .finally(enableButtons);
}

function container_renew(challenge_id) {
  var alert = resetAlert();

  fetch("/api/v1/containers/renew", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json",
      "CSRF-Token": init.csrfNonce,
    },
    body: JSON.stringify({ challenge_id: challenge_id }),
  })
    .then(function (r) {
      return r.json();
    })
    .then(function (data) {
      while (alert.firstChild) alert.removeChild(alert.firstChild);
      if (data.error) {
        alert.textContent = data.error;
        alert.classList.add("alert-danger");
      } else {
        view_container_info(challenge_id);
      }
    })
    .catch(function (err) {
      _setAlertText(alert, "Error renewing instance.");
      alert.classList.add("alert-danger");
      console.error("[Instance] Renew error:", err);
    })
    .finally(enableButtons);
}

function container_stop(challenge_id) {
  var alert = resetAlert();

  fetch("/api/v1/containers/stop", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json",
      "CSRF-Token": init.csrfNonce,
    },
    body: JSON.stringify({ challenge_id: challenge_id }),
  })
    .then(function (r) {
      return r.json();
    })
    .then(function (data) {
      while (alert.firstChild) alert.removeChild(alert.firstChild);
      if (data.error) {
        alert.textContent = data.error;
        alert.classList.add("alert-danger");
      } else {
        alert.textContent = "Instance terminated successfully.";
        hideUpdateBtns();
        showCreateBtn();
      }
    })
    .catch(function (err) {
      console.error("[Instance] Stop error:", err);
      _setAlertText(alert, "Error stopping instance.");
      alert.classList.add("alert-danger");
    })
    .finally(enableButtons);
}

// ── Init: inject UI if template block didn't render ──────────────────────────
(function () {
  var checkCount = 0;
  var maxChecks = 20;

  function _makeBtn(label, cls, challengeId, fn) {
    var btn = document.createElement("button");
    btn.type = "button";
    btn.className = "btn " + cls + " d-none";
    btn.addEventListener("click", function () {
      fn(challengeId);
    });
    var small = document.createElement("small");
    small.style.color = "white";
    small.textContent = label;
    btn.appendChild(small);
    return btn;
  }

  function checkAndInject() {
    checkCount++;

    var deploymentDiv = document.querySelector(".deployment-actions");
    if (deploymentDiv) {
      var challengeId = deploymentDiv.getAttribute("data-challenge-id");
      if (challengeId) {
        view_container_info(parseInt(challengeId));
        return true;
      }
    }

    var challengeWindow = document.getElementById("challenge-window");
    var challengeBody = challengeWindow
      ? challengeWindow.querySelector(".modal-body")
      : null;

    if (
      challengeBody &&
      !document.querySelector(".deployment-actions-injected")
    ) {
      var challengeId = null;
      if (
        window.challenge &&
        window.challenge.data &&
        window.challenge.data.id
      ) {
        challengeId = window.challenge.data.id;
      }
      if (
        !challengeId &&
        window.CTFd &&
        window.CTFd._internal &&
        window.CTFd._internal.challenge &&
        window.CTFd._internal.challenge.data
      ) {
        challengeId = window.CTFd._internal.challenge.data.id;
      }
      if (!challengeId) {
        var inp = document.getElementById("challenge-id");
        if (inp) challengeId = parseInt(inp.value);
      }
      if (!challengeId && challengeWindow) {
        var el = challengeWindow.querySelector("[data-challenge-id]");
        if (el) challengeId = parseInt(el.getAttribute("data-challenge-id"));
      }
      if (!challengeId) return false;

      // Build UI via DOM API
      var wrapper = document.createElement("div");
      wrapper.className = "mb-3 text-center deployment-actions-injected";

      var infoDiv = document.createElement("div");
      infoDiv.id = "deployment-info";
      infoDiv.className = "alert alert-primary";
      infoDiv.appendChild(_makeSpinner());

      var btnSpan = document.createElement("span");
      var createBtn = _makeBtn(
        "Fetch Instance",
        "btn-primary",
        challengeId,
        container_request,
      );
      createBtn.id = "create-chal";
      var extendBtn = _makeBtn(
        "Extend Time",
        "btn-info",
        challengeId,
        container_renew,
      );
      extendBtn.id = "extend-chal";
      var stopBtn = _makeBtn(
        "Terminate",
        "btn-danger",
        challengeId,
        container_stop,
      );
      stopBtn.id = "terminate-chal";
      btnSpan.append(createBtn, extendBtn, stopBtn);

      wrapper.append(infoDiv, btnSpan);

      var descSection = challengeBody.querySelector(".challenge-desc");
      if (descSection) {
        descSection.after(wrapper);
      } else {
        challengeBody.insertBefore(wrapper, challengeBody.firstChild);
      }
      view_container_info(challengeId);
      return true;
    }

    if (checkCount >= maxChecks) return true;
    return false;
  }

  if (checkAndInject()) return;

  var observer = new MutationObserver(function () {
    if (checkAndInject()) observer.disconnect();
  });
  observer.observe(document.body, { childList: true, subtree: true });
})();
