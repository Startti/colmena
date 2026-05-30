// slides_runtime.js — keyboard nav + fullscreen for HTML slides
// Activated only when layout_mode == "slides".
(function () {
  "use strict";
  var slides = document.querySelectorAll(".slide");
  if (slides.length === 0) return;
  var current = 0;
  var counter = document.getElementById("slide-counter");

  function goto(i) {
    if (i < 0) i = 0;
    if (i >= slides.length) i = slides.length - 1;
    current = i;
    slides[i].scrollIntoView({ behavior: "smooth", block: "start" });
    if (counter) counter.textContent = (i + 1) + " / " + slides.length;
  }

  document.addEventListener("keydown", function (e) {
    var key = e.key;
    if (key === "ArrowRight" || key === "PageDown" || key === " ") {
      e.preventDefault();
      goto(current + 1);
    } else if (key === "ArrowLeft" || key === "PageUp") {
      e.preventDefault();
      goto(current - 1);
    } else if (key === "f" || key === "F") {
      var deck = document.querySelector(".deck");
      if (!document.fullscreenElement && deck && deck.requestFullscreen) {
        deck.requestFullscreen();
      } else if (document.exitFullscreen) {
        document.exitFullscreen();
      }
    } else if (key === "Escape" && document.exitFullscreen) {
      document.exitFullscreen();
    }
  });

  if (counter) counter.textContent = "1 / " + slides.length;
})();
