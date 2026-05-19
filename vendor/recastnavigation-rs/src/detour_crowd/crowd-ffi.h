#pragma once

#include "DetourCommon.h"
#include "DetourCrowd.h"
#include "DetourLocalBoundary.h"
#include "DetourObstacleAvoidance.h"
#include "DetourPathCorridor.h"
#include "DetourPathQueue.h"
#include "DetourProximityGrid.h"

#include "rust/cxx.h"
#include "recastnavigation-rs/src/utils.h"

//
// dtLocalBoundary
//

inline void dtlb_reset(dtLocalBoundary& lb) { lb.reset(); }

inline void dtlb_update(
    dtLocalBoundary& lb,
    dtPolyRef ref,
    const float* pos,
    float collisionQueryRange,
    dtNavMeshQuery* navquery,
    const dtQueryFilter* filter
) {
    lb.update(ref, pos, collisionQueryRange, navquery, filter);
}

inline bool dtlb_isValid(const dtLocalBoundary& lb, dtNavMeshQuery* navquery, const dtQueryFilter* filter) {
    return lb.isValid(navquery, filter);
}

inline const float* dtlb_getCenter(const dtLocalBoundary& lb) { return lb.getCenter(); }

inline int dtlb_getSegmentCount(const dtLocalBoundary& lb) { return lb.getSegmentCount(); }
inline const float* dtlb_getSegment(const dtLocalBoundary& lb, int i) { return lb.getSegment(i); }

//
// dtCrowdAgent
//

inline bool dtca_getActive(const dtCrowdAgent& agent) { return agent.active; }
inline void dtca_setActive(dtCrowdAgent& agent, bool active) { agent.active = active; }

inline unsigned char dtca_getState(const dtCrowdAgent& agent) { return agent.state; }
inline void dtca_setState(dtCrowdAgent& agent, unsigned char state) { agent.state = state; }

inline bool dtca_getPartial(const dtCrowdAgent& agent) { return agent.partial; }
inline void dtca_setPartial(dtCrowdAgent& agent, bool partial) { agent.partial = partial; }

inline const dtPathCorridor* dtca_getCorridor(const dtCrowdAgent& agent) { return &agent.corridor; }
inline dtPathCorridor* dtca_getCorridor_mut(dtCrowdAgent& agent) { return &agent.corridor; }

inline const dtLocalBoundary* dtca_getBoundary(const dtCrowdAgent& agent) { return &agent.boundary; }
inline dtLocalBoundary* dtca_getBoundary_mut(dtCrowdAgent& agent) { return &agent.boundary; }

inline float dtca_getTopologyOptTime(const dtCrowdAgent& agent) { return agent.topologyOptTime; }
inline void dtca_setTopologyOptTime(dtCrowdAgent& agent, float topologyOptTime) { agent.topologyOptTime = topologyOptTime; }

inline const dtCrowdNeighbour* dtca_getNeis(const dtCrowdAgent& agent) { return agent.neis; }
inline dtCrowdNeighbour* dtca_getNeis_mut(dtCrowdAgent& agent) { return agent.neis; }

inline int dtca_getNneis(const dtCrowdAgent& agent) { return agent.nneis; }
inline void dtca_setNneis(dtCrowdAgent& agent, int nneis) { agent.nneis = nneis; }

inline float dtca_getDesiredSpeed(const dtCrowdAgent& agent) { return agent.desiredSpeed; }
inline void dtca_setDesiredSpeed(dtCrowdAgent& agent, float desiredSpeed) { agent.desiredSpeed = desiredSpeed; }

inline const float* dtca_getNpos(const dtCrowdAgent& agent) { return agent.npos; }
inline void dtca_setNpos(dtCrowdAgent& agent, const float* npos) { dtVcopy(agent.npos, npos); }

inline const float* dtca_getDisp(const dtCrowdAgent& agent) { return agent.disp; }
inline void dtca_setDisp(dtCrowdAgent& agent, const float* disp) { dtVcopy(agent.disp, disp); }

inline const float* dtca_getDvel(const dtCrowdAgent& agent) { return agent.dvel; }
inline void dtca_setDvel(dtCrowdAgent& agent, const float* dvel) { dtVcopy(agent.dvel, dvel); }

inline const float* dtca_getNvel(const dtCrowdAgent& agent) { return agent.nvel; }
inline void dtca_setNvel(dtCrowdAgent& agent, const float* nvel) { dtVcopy(agent.nvel, nvel); }

inline const float* dtca_getVel(const dtCrowdAgent& agent) { return agent.vel; }
inline void dtca_setVel(dtCrowdAgent& agent, const float* vel) { dtVcopy(agent.vel, vel); }

inline const dtCrowdAgentParams* dtca_getParams(const dtCrowdAgent& agent) { return &agent.params; }
inline dtCrowdAgentParams* dtca_getParams_mut(dtCrowdAgent& agent) { return &agent.params; }

inline const float* dtca_getCornerVerts(const dtCrowdAgent& agent) { return agent.cornerVerts; }
inline float* dtca_getCornerVerts_mut(dtCrowdAgent& agent) { return agent.cornerVerts; }

inline const unsigned char* dtca_getCornerFlags(const dtCrowdAgent& agent) { return agent.cornerFlags; }
inline unsigned char* dtca_getCornerFlags_mut(dtCrowdAgent& agent) { return agent.cornerFlags; }

inline const dtPolyRef* dtca_getCornerPolys(const dtCrowdAgent& agent) { return agent.cornerPolys; }
inline dtPolyRef* dtca_getCornerPolys_mut(dtCrowdAgent& agent) { return agent.cornerPolys; }

inline int dtca_getNcorners(const dtCrowdAgent& agent) { return agent.ncorners; }
inline void dtca_setNcorners(dtCrowdAgent& agent, int ncorners) { agent.ncorners = ncorners; }

inline unsigned char dtca_getTargetState(const dtCrowdAgent& agent) { return agent.targetState; }
inline void dtca_setTargetState(dtCrowdAgent& agent, unsigned char targetState) { agent.targetState = targetState; }

inline dtPolyRef dtca_getTargetRef(const dtCrowdAgent& agent) { return agent.targetRef; }
inline void dtca_setTargetRef(dtCrowdAgent& agent, dtPolyRef targetRef) { agent.targetRef = targetRef; }

inline const float* dtca_getTargetPos(const dtCrowdAgent& agent) { return agent.targetPos; }
inline void dtca_setTargetPos(dtCrowdAgent& agent, const float* targetPos) { dtVcopy(agent.targetPos, targetPos); }

inline dtPathQueueRef dtca_getTargetPathqRef(const dtCrowdAgent& agent) { return agent.targetPathqRef; }
inline void dtca_setTargetPathqRef(dtCrowdAgent& agent, dtPathQueueRef targetPathqRef) { agent.targetPathqRef = targetPathqRef; }

inline bool dtca_getTargetReplan(const dtCrowdAgent& agent) { return agent.targetReplan; }
inline void dtca_setTargetReplan(dtCrowdAgent& agent, bool targetReplan) { agent.targetReplan = targetReplan; }

inline float dtca_getTargetReplanTime(const dtCrowdAgent& agent) { return agent.targetReplanTime; }
inline void dtca_setTargetReplanTime(dtCrowdAgent& agent, float targetReplanTime) { agent.targetReplanTime = targetReplanTime; }
