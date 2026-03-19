// Reuse the same toggle functions from create.js
function toggleScoringType(val) {
    document.getElementById("standard-scoring").style.display = val === "standard" ? "" : "none";
    document.getElementById("dynamic-scoring").style.display = val === "dynamic" ? "" : "none";
}

function toggleBackendFields(val) {
    ["docker", "compose", "lxc", "vagrant"].forEach(function (b) {
        var el = document.getElementById("fields-" + b);
        if (el) el.style.display = b === val ? "" : "none";
    });
}

function toggleFlagMode(val) {
    var rf = document.getElementById("random-flag-fields");
    if (rf) rf.style.display = val === "random" ? "" : "none";
}

document.addEventListener("DOMContentLoaded", function () {
    var backendSelect = document.getElementById("backend-select");
    if (backendSelect) toggleBackendFields(backendSelect.value);
    // scoring-type-select initial state is set by server-rendered selected attributes
    var scoringSelect = document.getElementById("scoring-type-select");
    if (scoringSelect) toggleScoringType(scoringSelect.value);
});
