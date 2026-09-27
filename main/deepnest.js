/*!
 * Deepnest
 * Licensed under GPLv3
 */
 

	'use strict';

	// NOTE (2026-09): `DeepNest` is retained ONLY as the SVG importer
	// (`importsvg`/`getParts`/`toTree`). The GA/worker `start()` path below is
	// legacy and unused: the nesting engine is now ironnest
	// (src/geometry/engine-ironnest, native/ironnest-napi). The worker script it
	// used (`background.js`) has been removed.
	//
	// Loaded lazily: svgparser pulls in a browser SVGPathSeg polyfill that
	// touches `window`.
	var SvgParser = null;
	function svgParser() {
		if (!SvgParser) {
			SvgParser = require('./svgparser');
		}
		return SvgParser;
	}
	const ClipperLib = require('./util/clipper');
	const {GeometryUtil} = require('./util/geometryutil');
	
	/**
	 * 
	 * @param {EventTarget} eventEmitter 
	 * @param {Record<string, string|number>} configuration 
	 */
	function DeepNest(eventEmitter, configuration = {
		clipperScale: 10000000,
		curveTolerance: 0.3, 
		spacing: 0,
		rotations: 4,
		populationSize: 10,
		mutationRate: 10,
		threads: 4,
		placementType: 'gravity',
		mergeLines: true,
		timeRatio: 0.5,
		scale: 72,
		simplify: false
	}) {		
		
		var config = {...configuration};
		
		// list of imported files
		// import: {filename: 'blah.svg', svg: svgroot}
		this.imports = [];
		
		// list of all extracted parts
		// part: {name: 'part name', quantity: ...}
		this.parts = [];
		

		this.eventEmitter = eventEmitter;
		
		this.importsvg = function(filename, dirpath, svgstring, scalingFactor, dxfFlag){
			// parse svg
			// config.scale is the default scale, and may not be applied
			// scalingFactor is an absolute scaling that must be applied regardless of input svg contents
			var svg = svgParser().load(dirpath, svgstring, config.scale, scalingFactor);
			svg = svgParser().clean(dxfFlag);

			if(filename){
				this.imports.push({
					filename: filename,
					svg: svg
				});
			}
			
			const parts = this.getParts(svg.children, filename);
			this.parts.push(...parts)

			return parts;
		}
		
		this.config = function(c){
			// clean up inputs
			
			if(!c){
				return config;
			}
			
			if(c.curveTolerance && !GeometryUtil.almostEqual(parseFloat(c.curveTolerance), 0)){
				config.curveTolerance =  parseFloat(c.curveTolerance);
			}
			
			if('spacing' in c){
				config.spacing = parseFloat(c.spacing);
			}
			
			if(c.rotations && parseInt(c.rotations) > 0){
				config.rotations = parseInt(c.rotations);
			}
			
			if(c.populationSize && parseInt(c.populationSize) > 2){
				config.populationSize = parseInt(c.populationSize);
			}
			
			if(c.mutationRate && parseInt(c.mutationRate) > 0){
				config.mutationRate = parseInt(c.mutationRate);
			}
			
			if(c.threads && parseInt(c.threads) > 0){
				// max 8 threads
				config.threads = Math.min(parseInt(c.threads), 8);
			}
			
			if(c.placementType){
				config.placementType = String(c.placementType);
			}
			
			if(c.mergeLines === true || c.mergeLines === false){
				config.mergeLines = !!c.mergeLines;
			}
			
			if(c.simplify === true || c.simplify === false){
				config.simplify = !!c.simplify;
			}
			
			var n = Number(c.timeRatio);
			if(typeof n == 'number' && !isNaN(n) && isFinite(n)){
				config.timeRatio = n;
			}
			
			if(c.scale && parseInt(c.scale) > 0){
				config.scale = parseInt(c.scale);
			}

			svgParser().config({ tolerance: config.curveTolerance, endpointTolerance: c.endpointTolerance});
			
						
			return config;
		}
		
		this.pointInPolygon = function(point, polygon){
			// scaling is deliberately coarse to filter out points that lie *on* the polygon
			var p = this.svgToClipper(polygon, 1000);
			var pt = new ClipperLib.IntPoint(1000*point.x,1000*point.y);
			
			return ClipperLib.Clipper.PointInPolygon(pt, p) > 0;
		}
		
		this.getParts = function(paths, filename){

			var i, j;
			var polygons = [];
			
			var numChildren = paths.length;
			for(i=0; i<numChildren; i++){
			
				if(svgParser().polygonElements.indexOf(paths[i].tagName) < 0){
					continue;
				}
				
				// don't use open paths
				if(!svgParser().isClosed(paths[i], 2*config.curveTolerance)){
					continue;
				}
				
				var poly = svgParser().polygonify(paths[i]);
				poly = this.cleanPolygon(poly);

				// todo: warn user if poly could not be processed and is excluded from the nest
				if(poly && poly.length > 2 && Math.abs(GeometryUtil.polygonArea(poly)) > config.curveTolerance*config.curveTolerance){
					poly.source = i;
					polygons.push(poly);
				} else {
					console.warn('Excluding poly', poly, paths[i])
				}
			}
						
			// turn the list into a tree
			// root level nodes of the tree are parts
			toTree(polygons);
						
			function toTree(list, idstart){
				function svgToClipper(polygon){
					var clip = [];
					for(var i=0; i<polygon.length; i++){
						clip.push({X: polygon[i].x, Y: polygon[i].y});
					}
			
					ClipperLib.JS.ScaleUpPath(clip, config.clipperScale);
			
					return clip;
				}
				function pointInClipperPolygon(point, polygon){
					var pt = new ClipperLib.IntPoint(config.clipperScale*point.x,config.clipperScale*point.y);
					
					return ClipperLib.Clipper.PointInPolygon(pt, polygon) > 0;
				}
				var parents = [];
				var i,j,k;
				
				// assign a unique id to each leaf
				var id = idstart || 0;
				
				for(i=0; i<list.length; i++){
					var p = list[i];
					
					var ischild = false;
					for(j=0; j<list.length; j++){
						if(j==i){
							continue;
						}
						if(p.length < 2){
							continue;
						}
						var inside = 0;
						var fullinside = Math.min(10, p.length);
						
						// sample about 10 points
						var clipper_polygon = svgToClipper(list[j]);
						
						for(k=0; k<fullinside; k++){
							if(pointInClipperPolygon(p[k], clipper_polygon) === true){
								inside++;
							}
						}
						
						//console.log(inside, fullinside);
						
						if(inside > 0.5*fullinside){
							if(!list[j].children){
								list[j].children = [];
							}
							list[j].children.push(p);
							p.parent = list[j];
							ischild = true;
							break;
						}
					}
					
					if(!ischild){
						parents.push(p);
					}
				}
				
				for(i=0; i<list.length; i++){
					if(parents.indexOf(list[i]) < 0){
						list.splice(i, 1);
						i--;
					}
				}
				
				for(i=0; i<parents.length; i++){
					parents[i].id = id;
					id++;
				}
				
				for(i=0; i<parents.length; i++){
					if(parents[i].children){
						id = toTree(parents[i].children, id);
					}
				}
								
				return id;
			};
			
			// construct part objects with metadata
			var parts = [];
			var svgelements = Array.prototype.slice.call(paths);
			var openelements = svgelements.slice(); // elements that are not a part of the poly tree but may still be a part of the part (images, lines, possibly text..)
			
			for(i=0; i<polygons.length; i++){
				var part = {};
				part.polygontree = polygons[i];
				part.svgelements = [];
				
				var bounds = GeometryUtil.getPolygonBounds(part.polygontree);
				part.bounds = bounds;
				part.area = bounds.width*bounds.height;
				part.quantity = 1;
				part.filename = filename;
				
			  if (part.filename === 'BACKGROUND.svg') { part.sheet = true }
				
				// load root element
				part.svgelements.push(svgelements[part.polygontree.source]);
				var index = openelements.indexOf(svgelements[part.polygontree.source]);
				if(index > -1){
					openelements.splice(index,1);
				}
				
				// load all elements that lie within the outer polygon
				for(j=0; j<svgelements.length; j++){
					if(j != part.polygontree.source && findElementById(j, part.polygontree)){
						part.svgelements.push(svgelements[j]);
						index = openelements.indexOf(svgelements[j]);
						if(index > -1){
							openelements.splice(index,1);
						}
					}
				}
				
				parts.push(part);
			}
			
			function findElementById(id, tree){
				if(id == tree.source){
					return true;
				}
				
				if(tree.children && tree.children.length > 0){
					for(var i=0; i<tree.children.length; i++){
						if(findElementById(id, tree.children[i])){
							return true;
						}
					}
				}
				
				return false;
			}
						
			for(i=0; i<parts.length; i++){
				var part = parts[i];
				// the elements left are either erroneous or open
				// we want to include open segments that also lie within the part boundaries
				for(j=0; j<openelements.length; j++){
					var el = openelements[j];
					if(el.tagName == 'line'){
						var x1 = Number(el.getAttribute('x1'));
						var x2 = Number(el.getAttribute('x2'));
						var y1 = Number(el.getAttribute('y1'));
						var y2 = Number(el.getAttribute('y2'));
						var start = {x: x1, y: y1};
						var end = {x: x2, y: y2};
						var mid = {x: ((start.x+end.x)/2), y: ((start.y+end.y)/2)};

						if(this.pointInPolygon(start, part.polygontree) === true || 
							this.pointInPolygon(end, part.polygontree) === true ||
							this.pointInPolygon(mid, part.polygontree) === true ){
							part.svgelements.push(el);
							openelements.splice(j,1);
							j--;
						}
					}
					else if(el.tagName == 'image'){
						var x = Number(el.getAttribute('x'));
						var y = Number(el.getAttribute('y'));
						var width = Number(el.getAttribute('width'));
						var height = Number(el.getAttribute('height'));
						
						var mid = {x:x+(width/2) , y:y+(height/2)};
						
						var transformString = el.getAttribute('transform')
						if(transformString){
							var transform = svgParser().transformParse(transformString);
							if(transform){
								var transformed = transform.calc(mid.x, mid.y);
								mid.x = transformed[0];
								mid.y = transformed[1];
							}
						}
						// just test midpoint for images
						if(this.pointInPolygon(mid, part.polygontree) === true){
							part.svgelements.push(el);
							openelements.splice(j,1);
							j--;
						}
					}
					else if(el.tagName == 'path' || el.tagName == 'polyline'){
						var k;
						if(el.tagName == 'path'){
							var p = svgParser().polygonifyPath(el);
						}
						else{
							var p = [];
							for(k=0; k<el.points.length; k++){
								p.push({
									x: el.points[k].x,
									y: el.points[k].y
								});
							}
						}
						
						if(p.length < 2){
							continue;
						}
						
						var found = false;
						var next = p[1];
						for(k=0; k<p.length; k++){
							if(this.pointInPolygon(p[k], part.polygontree) === true){
								found = true;
								break;
							}
							
							if(k >= p.length-1){
								next = p[0];
							}
							else{
								next = p[k+1];
							}
							
							// also test for midpoints in case of single line edge case
							var mid = {
								x: (p[k].x+next.x)/2,
								y: (p[k].y+next.y)/2
							};
							if(this.pointInPolygon(mid, part.polygontree) === true){
								found = true;
								break;
							}
							
						}
						if(found){
							part.svgelements.push(el);
							openelements.splice(j,1);
							j--;
						}
					}
					else{
						// something went wrong
						//console.log('part not processed: ',el);
					}
				}
			}
			
			for(j=0; j<openelements.length; j++){
				var el = openelements[j];
				if(el.tagName == 'line' || el.tagName == 'polyline' || el.tagName == 'path'){
					el.setAttribute('class', 'error');
				}
			}
			
			return parts;
		};
		
		// returns a less complex polygon that satisfies the curve tolerance
		this.cleanPolygon = function(polygon){
			var p = this.svgToClipper(polygon);
			// remove self-intersections and find the biggest polygon that's left
			var simple = ClipperLib.Clipper.SimplifyPolygon(p, ClipperLib.PolyFillType.pftNonZero);
			
			if(!simple || simple.length == 0){
				return null;
			}
			
			var biggest = simple[0];
			var biggestarea = Math.abs(ClipperLib.Clipper.Area(biggest));
			for(var i=1; i<simple.length; i++){
				var area = Math.abs(ClipperLib.Clipper.Area(simple[i]));
				if(area > biggestarea){
					biggest = simple[i];
					biggestarea = area;
				}
			}

			// clean up singularities, coincident points and edges
			var clean = ClipperLib.Clipper.CleanPolygon(biggest, 0.01*config.curveTolerance*config.clipperScale);
			
			if(!clean || clean.length == 0){
				return null;
			}
			
			var cleaned = this.clipperToSvg(clean);
			
			// remove duplicate endpoints
			var start = cleaned[0];
			var end = cleaned[cleaned.length-1];
			if(start == end || (GeometryUtil.almostEqual(start.x,end.x) && GeometryUtil.almostEqual(start.y,end.y))){
				cleaned.pop();
			}
						
			return cleaned;
		}
		
		
		// converts a polygon from normal float coordinates to integer coordinates used by clipper, as well as x/y -> X/Y
		this.svgToClipper = function(polygon, scale){
			var clip = [];
			for(var i=0; i<polygon.length; i++){
				clip.push({X: polygon[i].x, Y: polygon[i].y});
			}
			
			ClipperLib.JS.ScaleUpPath(clip, scale || config.clipperScale);
			
			return clip;
		}
		
		this.clipperToSvg = function(polygon){
			var normal = [];
			
			for(var i=0; i<polygon.length; i++){
				normal.push({x: polygon[i].X/config.clipperScale, y: polygon[i].Y/config.clipperScale});
			}
			
			return normal;
		}
	}
	
module.exports = {
	DeepNest
}
