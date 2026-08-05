var downloader = {

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

	getCSSDataURI: function (x, uri, depth) {
		if (0 === uri.indexOf("data:"))
			return uri;

		var uriObj = registerURI(x, uri, depth);
		if (!uriObj.promise) {
			// ...
		}

		return uriObj.placeholder;
	},

	resolveRefs: function (elem, srcElem) {
		if (("LINK" === elem.tagName) && elem.href) {
			var str = (elem.type || "") + (elem.rel || "");
			var iconImage = str.includes("icon") || str.includes("image");

			if (!iconImage)
				elem.href = getCSSDataURI(null, elem.href, 0);
		}

		var attributes = elem.attributes;
		if (attributes)
			for (i = 0; i < attributes.length; i += 1)
				0 === attributes[i].name.toLowerCase().indexOf("on") && elem.removeAttribute(attributes[i].name);

// ...

		return elem;
	},

	cloneElement: function (src) {
		var result = src.cloneNode(!1);
		var child, childClone;

		result = resolveRefs(result, src);

		if (src.hasChildNodes())
			for (child = src.firstChild; child;)
				if (("none" !== (child.style ? child.style.display : "").toLowerCase())
					&& (8 !== child.nodeType) // not comment
					&& (-1 === ["BASE", "SCRIPT", "NOSCRIPT"].indexOf(child.tagName))
					&& (("LINK" !== child.tagName) || !child.rel || -1 !== child.rel.indexOf("stylesheet") || -1 !== child.rel.indexOf("icon"))
					&& ("IMG" !== child.tagName || 1 < child.width || 1 < child.height)
//				&& ("PARAM" !== child.tagName || "application/x-shockwave-flash" !== child.parentElement.type)
//				&& ("IFRAME" !== child.tagName || !child2.src || -1 === child.src.indexOf("chrome-extension://"))
				) {
					childClone = cloneElement(child);
					result.appendChild(childClone);
					child = child.nextSibling;
				}

		return result;
	},

	download_webpage: function () {
		var docClone = cloneElement(document.documentElement);

		docClone.outerHTML;
	},

};

dc.download_webpage();