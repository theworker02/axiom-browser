var log = [];
document.addEventListener('readystatechange', function () { log.push('readystatechange:' + document.readyState); });
document.addEventListener('DOMContentLoaded', function () { log.push('DOMContentLoaded:' + document.readyState); });
window.addEventListener('load', function () { log.push('load:' + document.readyState); });
