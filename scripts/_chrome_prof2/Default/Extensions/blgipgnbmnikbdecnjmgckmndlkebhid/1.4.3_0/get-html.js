var dc = {
    Deferred: function(e, r) {
        "use strict";
        var thisCopy = this;
        var t = e || new Promise(function(/*e*/resolutionFunc, /*t*/rejectionFunc) {
				thisCopy.resolve = function() {	thisCopy.timer && clearTimeout(thisCopy.timer), resolutionFunc() };
				thisCopy.reject = function() { thisCopy.timer && clearTimeout(thisCopy.timer), rejectionFunc && rejectionFunc() };
				r && (thisCopy.timer = setTimeout(dc.wrap(function() { thisCopy.time_out = !0, delete thisCopy.timer, thisCopy.resolve()}), 6e4))
            });

        this.promise = function() { return t  };
        this.clearTimer = function() { this.timer && clearTimeout(thisCopy.timer) };
        this.then = function(e) { return t.then(e) };
        this.done = function(e) { return t.then(e, e) };

		return this;
    },
    cloneTime: {},
    promises: [],
    URIs: {},
    uriCounter: 0,
    doc_prefix: "",
    analytics: [],
    iframesToProcess: [],
    postData: null,
    topWindow: window.self === window.top,
    output: {
        refs: [],
        origin: location.origin,
        currentSize: 0,
        hasError: !1
    },
    wrap: function(e, t) {
        "use strict";
        return function() {
            var t;
            try {
                return this.func.apply(this.context, this.args.concat(Array.prototype.slice.call(arguments, 0)))
            } catch (e) {
                throw e.handled || (e.handled = !0, t = "DCBrowser:Error:JS:" + e.stack.match(/get-html\.js:(\d*):(\d*)/)[0] + ":" + e.message.replace(/\s/g, "_").replace(/"\S*?"/g, ""), console.log(t), chrome.runtime.sendMessage({
                    progress_op: "html-blob",
                    main_op: OP,
                    error_analytics: t,
                    error: "web2pdfHTMLJSError"
                })), e
            }
        }.bind({
            func: e,
            context: t || {},
            args: Array.prototype.slice.call(arguments, 1)
        })
    },
    random: function() {
        "use strict";
        return "W" + Math.ceil(1e5 * Math.random()).toString()
    },
    log: function(e) {
        "use strict";
        DEBUG && console.log(e)
    },
    getDomain: function(e) {
        "use strict";
        var t = document.createElement("a");
        return t.href = e, t.origin
    },
    utf8ByteLength: function(e) {
        "use strict";
        if (!e) return 0;
        var t = encodeURI(e),
            r = t.match(/%/g);
        return r ? t.length - 2 * r.length : t.length
    },
    resolveURL: function(e, /*t*/uri) {
        "use strict";
        return e ?
			(0 === (uri = uri.trim()).search(/https?:\/\//) ? uri :
				(0 === uri.indexOf("//") ? uri :
					(0 === uri.indexOf("/") ? dc.getDomain(e) + uri : e + "/" + uri)))
			: uri;
    },
    registerURI: function(e, /*t*/uri, /*r*/depth) {
        "use strict";

        var c = (0 === depth) ? ("refs/" + dc.doc_prefix) : dc.doc_prefix;
        var n = "." + uri.replace(/[\?#]\S*/, "").split(".").pop();

		if (5 < n.length || 1 === n.length)
			(n = "");

		uri = dc.resolveURL(e, uri);

		if (!dc.URIs[uri])
		{
			dc.URIs[uri] = { placeholder: c + dc.uriCounter.toString() + n };
			dc.uriCounter += 1;
		}
        return dc.URIs[uri];
    },
    getType: function(e, t) {
        "use strict";
        var r = "image";
        return t.endsWith(".ttf") ? (dc.analytics.push("FONT_TTF"), r = "font") : t.endsWith(".otf") ? (dc.analytics.push("FONT_OTF"), r = "font") : t.endsWith(".woff") ? (dc.analytics.push("FONT_WOFF"), r = "font") : t.endsWith(".eot") && (dc.analytics.push("FONT_EOT"), r = "font"), "text/xml" === e || "application/vnd.ms-fontobject" === e ? (dc.analytics.push("FONT_EOT"), r = "font") : "font/woff2" === e && (dc.analytics.push("FONT_WOFF2"), r = "font"), e.startsWith("image/svg") && (r = "svg_image", dc.analytics.push("SVG_IMAGE")), r
    },
    readImage: function(/*e*/request, /*t*/URIObj) {
        "use strict";
        var r;
		if (request.status < 400)
		{
			URIObj.type = dc.getType(request.response.type, URIObj.placeholder);

			if (-1 !== EXCLUDE.indexOf(URIObj.type))
			{
				URIObj.data = null;
				URIObj.promise.resolve();
			}  else
			{
				(r = new FileReader).onloadend = dc.wrap(function(e) {
					URIObj.promise.time_out || (URIObj.data = e.target.result, URIObj.promise.resolve())
				}),
				r.readAsDataURL(request.response);
			}
		}
		else
		{
			dc.log("FAILED to load: " + e.responseURL);
			dc.log("Placeholder: " + t.placeholder);
			URIObj.promise.resolve();
		}
    },
    getDataURI: function(e, /*t*/url, /*r*/depth) {
        "use strict";
        if (0 === url.indexOf("data:"))
        	return url;
        if (!url)
        	return url;
        var request, n, s = dc.registerURI(e, url, depth);

        if (!s.promise)
		{
			request = new XMLHttpRequest;
			s.promise = new dc.Deferred(null, !0);
			dc.promises.push(s.promise);
			n = dc.resolveURL(e, url).replace(/^https?:/, "");
			request.open("GET", n, !0);
			request.responseType = "blob";
			request.onload = dc.wrap(function(e) {
				s.promise.time_out || dc.readImage(this, s)
			}, request);
			request.send();
		}

        return s.placeholder;
    },
    replaceURL: function(/*e*/url, /*t*/dataUrl, /*r*/data) {
        "use strict";
        var c = new RegExp(url.replace(/([\.\^\$\*\+\?\(\)\[\{\\\|])/g, "\\$1"), "g");
        return data.replace(c, dataUrl)
    },
    replaceCssRefs: function(/*e*/url, /*t*/data, /*r*/depth) {
        "use strict";
        var c, n, s, a, o;
        var i = (o = (a = url).split("/")).length < 4 ? a : o.slice(0, -1).join("/");
        var d = [];
        var l = /([\s\S]{0,10})url\s*\(([\s\S]*?)\)/gm;
        for (data = data.replace(/\/\*[\s\S]*?\*\//gm, ""), c = l.exec(data); c;)
        	n = (s = c[2]).replace(/('|")/g, ""),
			s && 0 !== n.indexOf("data:") && d.push({
            	url: n,
            	imprt: -1 !== c[1].indexOf("@import"),
            	originalURL: s}),
			c = l.exec(data);
        return d.forEach(dc.wrap(function(e) {
			data = e.imprt ?
				"acro-html" === OP ? dc.replaceURL(e.originalURL, dc.resolveURL(i, e.url), data) : dc.replaceURL(e.originalURL, dc.getCSSDataURI(i, e.url, depth + 1), data)
				: "acro-html" === OP ? dc.replaceURL(e.originalURL, dc.resolveURL(i, e.url), data) : dc.replaceURL(e.originalURL, dc.getDataURI(i, e.url, depth + 1), data)
        })), data
    },
    getCSSDataURI: function(e, /*t*/sourceHref, /*r*/depth) {
        "use strict";
        if (0 === sourceHref.indexOf("data:"))
        	return sourceHref;

        var /*c*/request, n, s = dc.registerURI(e, sourceHref, depth);

        if (!s.promise)
		{
			s.promise = new dc.Deferred(null, !0);
			dc.promises.push(s.promise);
			request = new XMLHttpRequest;
			n = dc.resolveURL(e, sourceHref).replace(/^https?:/, "");
			request.open("GET", n, !0);
			request.responseType = "blob";
			request.onload = dc.wrap(function(e)
				{
					var t;
					if (this.status < 400)
					{
						if ("text/css" !== this.response.type)
							return void dc.readImage(this, s);

						(t = new FileReader).onloadend = function(e)
						{
							s.type = "css";
							s.promise.clearTimer();
							if (!s.promise.time_out)
							{
								s.data = dc.replaceCssRefs(n, e.target.result, depth);
								s.promise.resolve()
							}
						};

						t.readAsText(this.response);
					} else
					{
						dc.log("CSS FAILED to load: " + c.responseURL + "   Placeholder: " + s.placeholder), s.promise.resolve()
					}
				}, request);
			request.send();
		}

        return s.placeholder;
    },
    processStyleAttr: function(e) {
        "use strict";
        var t = e.getAttribute ? e.getAttribute("style") : "";
        t && e.setAttribute("style", dc.replaceCssRefs(document.location.origin, t, -1))
    },
    resolveRefs: function(/*t*/elem, /*e*/src) {
        "use strict";
        var i, c, /*n*/attributes, s, str;
        if ("LINK" === elem.tagName
			&& elem.href
			&& ((str = (elem.type || "") + (elem.rel || "")).includes("icon") || str.includes("image") ?
				dc.log("skipped icon image") :
			elem.href = dc.getCSSDataURI(null, elem.href, 0)),
			attributes = src.attributes)

            for (i = 0; i < attributes.length; i += 1)
            	0 === attributes[i].name.toLowerCase().indexOf("on") && elem.removeAttribute(attributes[i].name);

		if (-1 !== ["IMG", "INPUT"].indexOf(elem.tagName))
		{
			elem.dataset._html_to_pdf_src_ = dc.getDataURI(null, elem.src, 0);
			elem.removeAttribute("src");
		}

		if (("INPUT" === elem.tagName) && ("file" !== elem.type))
			elem.setAttribute("value", src.value);

		if (("INPUT" === elem.tagName) && (("radio" === elem.type) || ("checkbox" === elem.type)))
		{
			elem.removeAttribute("checked");
			if (src.checked)
				elem.setAttribute("checked", "checked");
		}

		if ("OPTION" === elem.tagName)
		{
			elem.removeAttribute("selected");
			if (src.selected)
				elem.setAttribute("selected", "selected");
		}

		if ("EMBED" === elem.tagName)
		{
			elem.setAttribute("src", elem.src);
		}

		if ("IFRAME" === elem.tagName || "FRAME" === elem.tagName)
		{
            for (i = 0; i < window.frames.length; i += 1)
            	if (window.frames[i] === src.contentWindow)
                	 if (src.contentWindow.WINDOW_ID)
					 {
					 	dc.iframesToProcess[i] = { index: i };
					 	elem.src = src.contentWindow.WINDOW_ID + ".html";
					 } else
					 {
					 	elem.src = "about:blank";
					 }

			elem.name && 256 < elem.name.length && (elem.name = "")
        }

		if ("A" === elem.tagName)
		{
			s = elem.getAttribute("href");
			if (s && (0 === s.indexOf("/")))
				elem.setAttribute("href", elem.href);
		}

		if ((3 === elem.nodeType) && (src.parentNode) && ("style" === src.parentNode.tagName.toLowerCase()))
		{
			elem.textContent = dc.replaceCssRefs(document.location.origin, elem.textContent, -1);
		}

        if ("CANVAS" === elem.tagName)
        {
            c = document.createElement("IMG");
            try {
                c.src = src.toDataURL("image/png")
            } catch (e) {
                dc.analytics.push("TAINTED_CANVAS"), (c = document.createElement("DIV")).className = elem.className
            }
            elem = c;
            dc.analytics.push("CANVAS");
        }

		if ("image" === elem.tagName)
		{
			(function(el) {
				var t, attr, attrs = el.attributes,
					n = attrs.length;
				for (i = 0; i < n; i += 1)
					if (("href" === (attr = attrs[i]).localName) || ("src" === attr.localName))
						el.setAttribute(attr.name, dc.getDataURI(null, attr.value, -1))
			})(elem)
		}

		dc.processStyleAttr(elem);

        return elem;
    },
    getDocType: function(e) {
        "use strict";
        var t, r = e.doctype;

        function c(e) {
            return e.replace(/[&<>]/g, function(e) {
                return {
                    "&": "&amp;",
                    "<": "&lt;",
                    ">": "&gt;"
                } [e]
            })
        }
        return null === r ? "" : (t = "<!DOCTYPE " + r.name, r.publicId ? t += ' PUBLIC "' + c(r.publicId) + '"' : r.systemId && (t += " SYSTEM"), c(r.systemId) && (t += ' "' + r.systemId + '"'), t + ">")
    },
    htmlTree: function(sourceElement) {
        "use strict";
        var clonedElement, child, childClone, child2;
		clonedElement = sourceElement.cloneNode(!1);
		clonedElement = dc.resolveRefs(clonedElement, sourceElement);
        if (sourceElement.hasChildNodes())
            for (child = sourceElement.firstChild; child;)
            	"none" === ((child2 = child).style ? child2.style.display : "").toLowerCase()
				|| 8 === child2.nodeType
				|| -1 !== ["BASE", "SCRIPT", "NOSCRIPT"].indexOf(child2.tagName)
				|| "LINK" === child2.tagName && child2.rel && -1 === child2.rel.indexOf("stylesheet") && -1 === child2.rel.indexOf("icon")
				|| "IMG" === child2.tagName && 1 === child2.width && 1 === child2.height
				|| "PARAM" === child2.tagName && "application/x-shockwave-flash" === child2.parentElement.type && (dc.topWindow ? dc.analytics.push("FLASH") : dc.analytics.push("FLASH_IN_IFRAME"), "html-blob" === OP)
				|| "IFRAME" === child2.tagName && child2.src && 0 === child2.src.indexOf("chrome-extension://")
				|| (childClone = dc.htmlTree(child), clonedElement.appendChild(childClone)),
					child = child.nextSibling;
        return clonedElement
    },
    processIframes: function() {
        "use strict";
        var e, t;
        for (e = 0; e < dc.iframesToProcess.length; e += 1) {
			if ((t = dc.iframesToProcess[e]) && !t.promise) {
				t.promise = new dc.Deferred;
				dc.promises.push(t.promise);
				{	index: e	};
				chrome.runtime.sendMessage({
					main_op: "relay-msg",
					index: e,
					frameID: window.frames[e].WINDOW_ID,
					parentID: window.WINDOW_ID,
					tabId: TABID
				});
			}
		}
	},
    receiveIframe: function(e) {
        "use strict";
        if ("serialize_iframe" === e.html_op && e.tabId === TABID && e.parentID === window.WINDOW_ID) {
            dc.analytics = dc.analytics.concat(e.content_analytics), dc.output.refs = dc.output.refs.concat(e.refs);
            var t = 0;
            e.refs.forEach(dc.wrap(function(e) {
                t += e.data ? e.data.length : 0
            })), dc.output.currentSize += t, dc.iframesToProcess[e.index].promise.resolve()
        }
    },
	_serialize: function(n) {
		"use strict";
		var /*s*/elem, t, r = new dc.Deferred;
		dc.doc_prefix = n.frameID;
		elem = dc.htmlTree(document.documentElement);
		t = dc.promises.length;

		(function e()
		{
			dc.Deferred.all(dc.promises).done(dc.wrap(function()
			{
				dc.processIframes();
				dc.promises.length === t ? r.resolve() : (t = dc.promises.length, e())
			}));
			return r.promise();
		})().then(dc.wrap(function() {
			var e, t, r, c;
			for (e in dc.output.html = dc.getDocType(document) + elem.outerHTML.replace(/data\-_html_to_pdf_src_=/gm, "src="),
				dc.output.html = dc.output.html.replace(/[\u00A0-\uFFFF]/g, function (e) { return "&#" + e.charCodeAt(0) + ";" }),
				dc.output.currentSize += dc.output.html.length,
				dc.URIs)
				if (dc.URIs.hasOwnProperty(e))
				{
					if (dc.output.currentSize > 1048576 * maxSize)
					{
						t = "web2pdfHTMLTooLarge", dc.output.hasError = !0;
						break
					}
					dc.output.refs.push({
						placeholder: dc.URIs[e].placeholder,
						type: dc.URIs[e].type,
						data: dc.URIs[e].data
					}), dc.URIs[e].data && (dc.output.currentSize += dc.URIs[e].data.length)
				}
			r = dc.analytics.filter(function (e, t, r) {
				return t === dc.analytics.indexOf(e)
			}), dc = (dc.topWindow ? (c = ((new Date).getTime() - dc.cloneTime.start) / 100, chrome.extension.sendMessage({
				progress_op: "html-blob",
				main_op: OP,
				blob: dc.output,
				content_analytics: r,
				cloneTiming: c,
				error: t,
				error_analytics: t
			})) : (dc.output.refs.push({
				placeholder: window.WINDOW_ID + ".html",
				type: "html",
				data: dc.output.html
			}), dc.output.currentSize += dc.output.html.length, dc.output.currentSize > 1048576 * maxSize && (t = "web2pdfHTMLTooLarge"), c = ((new Date).getTime() - dc.cloneTime.start) / 100, chrome.runtime.sendMessage({
				tabid: TABID,
				main_op: "relay-msg",
				complete: !0,
				index: n.index,
				refs: dc.output.refs,
				frameID: window.WINDOW_ID,
				parentID: n.parentID,
				content_analytics: r,
				cloneTiming: c,
				error: t,
				error_analytics: t
			})), null)
			}))
	}
};

function receiveIframe(e) {
    "use strict";
    dc.wrap(dc.receiveIframe)(e)
}

function serialize(frameInfo) {
    "use strict";
	frameInfo.frameID === window.WINDOW_ID && window.setTimeout(dc.wrap(dc._serialize), 0, frameInfo)
}

dc.Deferred.all = function(e) {
    "use strict";
    return new dc.Deferred(Promise.all(e))
};
window.WINDOW_ID = dc.random();
dc.topWindow && (dc.cloneTime.start = new Date, serialize({frameID: window.WINDOW_ID}));